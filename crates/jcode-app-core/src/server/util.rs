use crate::build;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Default embedding idle unload threshold (15 minutes).
const EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS: u64 = 15 * 60;

pub(crate) fn debug_control_allowed() -> bool {
    // Check config file setting
    if crate::config::config().display.debug_socket {
        return true;
    }
    if std::env::var("JCODE_DEBUG_CONTROL")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return true;
    }
    // Check for file-based toggle (allows enabling without restart)
    if let Ok(jcode_dir) = crate::storage::jcode_dir()
        && jcode_dir.join("debug_control").exists()
    {
        return true;
    }
    false
}

pub(crate) fn embedding_idle_unload_secs() -> u64 {
    std::env::var("JCODE_EMBEDDING_IDLE_UNLOAD_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS)
}

pub(crate) async fn get_shared_mcp_pool(
    cell: &OnceCell<Arc<crate::mcp::SharedMcpPool>>,
) -> Arc<crate::mcp::SharedMcpPool> {
    cell.get_or_init(|| async { Arc::new(crate::mcp::SharedMcpPool::from_default_config()) })
        .await
        .clone()
}

pub(crate) fn server_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    build::shared_server_update_candidate(is_selfdev_session)
}

/// Resolve the binary the reload should actually exec into, with a hard
/// no-downgrade guard.
///
/// `server_update_candidate` can legitimately return an *older* binary (e.g. a
/// `shared-server` channel that an update never advanced, or a leftover self-dev
/// promotion synced from another machine). A forced reload bypasses
/// `server_has_newer_binary`, so without this guard it would silently exec into
/// that older binary and downgrade every connected client.
///
/// We never block a same-or-newer candidate (so self-dev builds, which are
/// freshly written and therefore newer by mtime, still apply). When the
/// candidate is *strictly older* than the running executable we refuse it and
/// re-exec into the current executable instead: same code, fresh process and
/// socket handoff, but no downgrade. Any mtime uncertainty is treated as "do
/// not downgrade".
///
/// Crucially, the candidate is the *newest* reload candidate across BOTH
/// self-dev flavors, not just the one matching `is_selfdev_session`. This keeps
/// the reload target consistent with `server_has_newer_binary`, which also scans
/// both flavors. Without this, a self-dev/canary daemon whose `shared-server`
/// channel is pinned to an *old* self-dev build would advertise
/// `server_has_update = true` (the normal-flavor probe self-heals to the freshly
/// installed release) yet reload into that same old pinned build -> the server
/// reports an update it can never apply, so the client upgrades while the server
/// stays stale and the auto-reload loops until it is suppressed. Selecting the
/// newest candidate across flavors still preserves a deliberately-pinned self-dev
/// build whenever that build is the freshest one on disk (the case the pin is
/// meant to protect).
pub(crate) fn reload_exec_target(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    let candidate = newest_reload_candidate(is_selfdev_session)?;
    // On Linux a self-dev rebuild rewrites the running binary in place (a dirty
    // build reuses the same `versions/<hash>` path), which unlinks the running
    // inode. `current_exe()` then resolves `/proc/self/exe` to a path with a
    // trailing " (deleted)" marker that is NOT a real file. If we keep that
    // marker we (a) fail the "same binary" fast-path below, (b) read no mtime so
    // the freshly-built candidate looks like a downgrade, and (c) fall back to
    // re-execing the bogus " (deleted)" path, which does not exist -> the server
    // exits without a replacement and strands every connected client. Strip the
    // marker so we compare against (and can re-exec) the real on-disk path.
    let current_exe = std::env::current_exe().ok().map(strip_deleted_suffix);

    let candidate_canonical = canonicalize_or(candidate.0.clone());
    let current_canonical = current_exe.as_ref().map(|p| canonicalize_or(p.clone()));

    let current_mtime = current_exe
        .as_ref()
        .map(|p| p.as_path())
        .and_then(binary_mtime);
    // A wrapper's own mtime says nothing about the code it runs; compare the
    // `.bin` behind it so the downgrade guard sees real binary ages.
    let candidate_mtime = binary_mtime(comparison_path(candidate_canonical.clone()).as_path());

    match guarded_reload_target(
        candidate.clone(),
        candidate_canonical.as_path(),
        current_exe.as_deref(),
        current_canonical.as_deref(),
        current_mtime,
        candidate_mtime,
    ) {
        ReloadTargetDecision::UseCandidate(target) => Some(target),
        ReloadTargetDecision::DowngradeBlockedUseCurrent(target) => {
            // Never strand clients by re-execing a binary that is gone from disk.
            // If the running exe was unlinked (e.g. an in-place rebuild) but the
            // candidate still exists, prefer the candidate over refusing to
            // reload. The candidate may be older, but a live downgrade beats a
            // dead server with no replacement.
            if !target.0.exists() && candidate_canonical.exists() {
                crate::logging::warn(&format!(
                    "reload downgrade guard: current binary {:?} is missing on disk; falling back to candidate {:?} to avoid stranding clients",
                    target.0, candidate.0,
                ));
                return Some(candidate);
            }
            crate::logging::warn(&format!(
                "reload downgrade guard: refusing to exec into older candidate; re-execing current binary {:?} instead",
                target.0,
            ));
            Some(target)
        }
        ReloadTargetDecision::DowngradeUnverifiable(target) => {
            crate::logging::warn(&format!(
                "reload downgrade guard: older candidate {:?} detected but current exe is unavailable; proceeding with candidate",
                target.0,
            ));
            Some(target)
        }
    }
}

fn binary_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Pick the newest reload candidate across BOTH self-dev flavors.
///
/// The session's own flavor (`is_selfdev_session`) is evaluated first so it wins
/// any exact-mtime tie, preserving self-dev semantics: a deliberately-pinned
/// self-dev `shared-server` build is honored whenever it is at least as fresh as
/// the other flavor's candidate. The other flavor only wins when it is
/// *strictly newer*, which is exactly the situation that makes
/// `server_has_newer_binary` report an update (e.g. `/update` installed a newer
/// release while the self-dev pin stayed on an older build).
fn newest_reload_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    let ordered = [
        server_update_candidate(is_selfdev_session),
        server_update_candidate(!is_selfdev_session),
    ];
    let with_mtimes = ordered.into_iter().flatten().map(|candidate| {
        let canonical = canonicalize_or(candidate.0.clone());
        // Freshness of a launcher wrapper is the freshness of the `.bin` it
        // exec's, not of the wrapper file written moments later.
        let mtime = binary_mtime(comparison_path(canonical.clone()).as_path());
        (candidate, canonical, mtime)
    });
    pick_newest_candidate(with_mtimes)
}

/// Pure, order-sensitive "newest candidate" selection used by
/// [`newest_reload_candidate`]. Candidates are provided in *preference order*
/// (the session's own flavor first). A later candidate only displaces an earlier
/// one when it is provably, strictly newer by mtime, so equal/unknown mtimes
/// never demote the higher-preference flavor (protecting a self-dev pin on a
/// tie). Canonical-path duplicates are collapsed to the first occurrence.
fn pick_newest_candidate(
    candidates: impl IntoIterator<
        Item = (
            (PathBuf, &'static str),
            PathBuf,
            Option<std::time::SystemTime>,
        ),
    >,
) -> Option<(PathBuf, &'static str)> {
    let mut best: Option<((PathBuf, &'static str), Option<std::time::SystemTime>)> = None;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for (candidate, canonical, mtime) in candidates {
        if !seen.insert(canonical) {
            continue;
        }
        let replace = match (&best, mtime) {
            (None, _) => true,
            (Some((_, Some(best_mtime))), Some(new_mtime)) => new_mtime > *best_mtime,
            (Some((_, None)), Some(_)) => true,
            (Some(_), None) => false,
        };
        if replace {
            best = Some((candidate, mtime));
        }
    }
    best.map(|(candidate, _)| candidate)
}

#[derive(Debug)]
enum ReloadTargetDecision {
    UseCandidate((PathBuf, &'static str)),
    DowngradeBlockedUseCurrent((PathBuf, &'static str)),
    DowngradeUnverifiable((PathBuf, &'static str)),
}

/// Pure no-downgrade decision used by [`reload_exec_target`]. A candidate is
/// accepted unless it is strictly older than (or not provably as new as) the
/// running executable, in which case we prefer re-execing the current binary.
fn guarded_reload_target(
    candidate: (PathBuf, &'static str),
    candidate_canonical: &Path,
    current_exe: Option<&Path>,
    current_canonical: Option<&Path>,
    current_mtime: Option<std::time::SystemTime>,
    candidate_mtime: Option<std::time::SystemTime>,
) -> ReloadTargetDecision {
    // Reloading into the same binary is always fine; no version question.
    if current_canonical == Some(candidate_canonical) {
        return ReloadTargetDecision::UseCandidate(candidate);
    }

    let candidate_is_strictly_older = match (current_mtime, candidate_mtime) {
        (Some(current), Some(cand)) => cand < current,
        // Unknown mtimes: be conservative and treat as a potential downgrade so
        // we never silently swap to an unverifiable binary on a forced reload.
        _ => true,
    };

    if !candidate_is_strictly_older {
        return ReloadTargetDecision::UseCandidate(candidate);
    }

    match current_exe {
        Some(current_exe) => ReloadTargetDecision::DowngradeBlockedUseCurrent((
            current_exe.to_path_buf(),
            "current-exe (downgrade-guard)",
        )),
        None => ReloadTargetDecision::DowngradeUnverifiable(candidate),
    }
}

fn canonicalize_or(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// Strip the Linux `/proc/self/exe` " (deleted)" marker that appears when the
/// running binary has been unlinked or replaced in place. The marker is part of
/// the readlink target, not the real filename, so removing it recovers the path
/// that may now point at the freshly written replacement binary.
fn strip_deleted_suffix(path: PathBuf) -> PathBuf {
    const DELETED_MARKER: &str = " (deleted)";
    if let Some(stripped) = path.to_str().and_then(|s| s.strip_suffix(DELETED_MARKER)) {
        return PathBuf::from(stripped);
    }
    path
}

/// Resolve a launcher wrapper script to the platform binary it exec's.
///
/// Linux compat releases install the real binary as `jcode-<target>.bin` plus a
/// tiny `jcode` sh wrapper that exec's it (see `scripts/build_linux_compat.sh`).
/// Channel symlinks point at the *wrapper*, while the running server is the
/// *`.bin`* the wrapper exec'd into, so the two never compare path-equal and
/// their mtimes are written at different instants during install. Mtime or
/// identity comparisons must therefore look *through* the wrapper to the `.bin`
/// beside it; exec targets keep using the wrapper so it can still set up
/// `LD_LIBRARY_PATH` for bundled libraries.
///
/// Returns `None` when `path` is not a wrapper script (e.g. it is the real
/// binary already) or the target `.bin` cannot be identified.
fn launcher_wrapper_target(path: &Path) -> Option<PathBuf> {
    // Wrappers are a few hundred bytes; never read a real multi-MB binary.
    const WRAPPER_MAX_LEN: u64 = 4096;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > WRAPPER_MAX_LEN {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    if !content.starts_with("#!") {
        return None;
    }
    let dir = path.parent()?;

    // Preferred: parse the `exec "$self_dir/<name>.bin" "$@"` line the build
    // script emits.
    let parsed = content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("exec ")?;
        let (_, after) = rest.split_once("$self_dir/")?;
        let name = after.split('"').next()?;
        name.ends_with(".bin").then(|| dir.join(name))
    });
    if let Some(target) = parsed
        && target.is_file()
    {
        return Some(target);
    }

    // Fallback for a wrapper whose text changed shape: a single sibling
    // platform binary is unambiguous.
    let stem = crate::build::binary_stem();
    let mut siblings = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "bin")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(stem))
                && p.is_file()
        });
    let first = siblings.next()?;
    siblings.next().is_none().then_some(first)
}

/// Path whose mtime/identity represents the *code* at `path`: the `.bin` behind
/// a launcher wrapper, or `path` itself otherwise. See [`launcher_wrapper_target`]
/// for why comparing the wrapper directly reports phantom updates forever.
fn comparison_path(path: PathBuf) -> PathBuf {
    launcher_wrapper_target(&path).unwrap_or(path)
}

pub(crate) fn git_common_dir_for(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        let dotgit = dir.join(".git");
        if dotgit.is_dir() {
            return Some(canonicalize_or(dotgit));
        }
        if dotgit.is_file() {
            let content = std::fs::read_to_string(&dotgit).ok()?;
            let gitdir_line = content
                .lines()
                .find(|line| line.trim_start().starts_with("gitdir:"))?;
            let raw = gitdir_line
                .trim_start()
                .trim_start_matches("gitdir:")
                .trim();
            if raw.is_empty() {
                return None;
            }
            let gitdir = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                dir.join(raw)
            };
            let gitdir = canonicalize_or(gitdir);
            // Worktree gitdir looks like: <repo>/.git/worktrees/<name>
            if let Some(parent) = gitdir.parent()
                && parent.file_name().and_then(|s| s.to_str()) == Some("worktrees")
                && let Some(common) = parent.parent()
            {
                return Some(canonicalize_or(common.to_path_buf()));
            }
            return Some(gitdir);
        }
        current = dir.parent();
    }
    None
}

pub(crate) fn swarm_id_for_dir(dir: Option<PathBuf>) -> Option<String> {
    if let Ok(sw_id) = std::env::var("JCODE_SWARM_ID") {
        let trimmed = sw_id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let dir = dir?;
    if let Some(git_common) = git_common_dir_for(&dir) {
        return Some(git_common.to_string_lossy().to_string());
    }
    Some(dir.to_string_lossy().to_string())
}

/// Decide whether any reload candidate is *provably* newer than the running
/// server binary.
///
/// This is intentionally conservative. An earlier version reported "update
/// available" whenever the mtime comparison was inconclusive (e.g. a metadata
/// read failed) as long as the candidate path differed from the running exe.
/// On some systems that fallback fired permanently, so the client would
/// auto-reload the server, the server would exec into the candidate, and the
/// freshly-exec'd server would again report an update -> an infinite reload
/// loop that flickers the terminal (see issue #277).
///
/// We now only report an update when we can read both mtimes and the candidate
/// is strictly newer than the running binary. Any uncertainty suppresses the
/// auto-reload signal so it can never wedge the client into a loop.
fn newer_binary_available(
    current_mtime: Option<std::time::SystemTime>,
    current_canonical: Option<&Path>,
    candidates: impl IntoIterator<Item = (PathBuf, Option<std::time::SystemTime>)>,
) -> bool {
    let Some(current_time) = current_mtime else {
        crate::logging::warn(
            "server_has_newer_binary: current executable mtime unavailable; suppressing auto-reload update signal",
        );
        return false;
    };

    candidates.into_iter().any(|(candidate, candidate_mtime)| {
        // Reloading into ourselves is never an "update".
        if current_canonical == Some(candidate.as_path()) {
            return false;
        }

        match candidate_mtime {
            Some(candidate_time) => candidate_time > current_time,
            None => {
                crate::logging::warn(&format!(
                    "server_has_newer_binary: candidate mtime unavailable for {}; suppressing auto-reload update signal",
                    candidate.display()
                ));
                false
            }
        }
    })
}

pub(crate) fn server_has_newer_binary() -> bool {
    // Directional check only: report an update solely when a reload *candidate*
    // binary is strictly newer than the binary we are running.
    //
    // We deliberately do NOT treat "my version differs from the installed
    // channel markers" as "I am outdated". That conflated *different* with
    // *older* and caused a real regression (issue #291): a newer self-dev /
    // shared-server daemon (e.g. v0.17.23-dev) running alongside an older
    // release client would be told to "reload" and downgrade itself, because
    // its git hash no longer matched the `current`/`stable` channel markers
    // after a release build moved them. It also fed the reload-loop family from
    // issue #277, since a server that merely "differs" can never make the
    // difference go away by reloading.
    //
    // `UPDATE_SEMVER` is the base Cargo version for every dev build, so it
    // cannot order two dev builds; binary mtime is the only robust, directional
    // signal we have. `newer_binary_available` compares candidate mtimes against
    // the running binary, excludes reloading into ourselves, and treats any
    // uncertainty (unreadable mtime) as "no update".
    //
    // Strip the Linux " (deleted)" marker (see `strip_deleted_suffix`) so an
    // in-place rebuild does not make the running binary's mtime unreadable and
    // suppress a legitimate update signal.
    let current_exe = std::env::current_exe().ok().map(strip_deleted_suffix);
    let current_mtime = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let current_canonical = current_exe
        .as_ref()
        .map(|path| comparison_path(canonicalize_or(path.clone())));

    // Compare against the `.bin` behind a launcher wrapper, not the wrapper
    // itself: the installer writes the wrapper a moment after the binary, so the
    // wrapper's mtime is always "newer" than the very code it exec's, and a
    // wrapper path can never compare equal to the running `.bin`.
    let mut candidates = HashSet::new();
    for is_selfdev_session in [false, true] {
        if let Some((candidate, _label)) = server_update_candidate(is_selfdev_session) {
            candidates.insert(comparison_path(canonicalize_or(candidate)));
        }
    }

    let candidates_with_mtimes = candidates.into_iter().map(|candidate| {
        let candidate_mtime = std::fs::metadata(&candidate)
            .ok()
            .and_then(|m| m.modified().ok());
        (candidate, candidate_mtime)
    });

    newer_binary_available(
        current_mtime,
        current_canonical.as_deref(),
        candidates_with_mtimes,
    )
}

/// Server identity for multi-server support
#[derive(Debug, Clone)]
pub struct ServerIdentity {
    /// Full server ID (e.g., "server_blazing_1705012345678")
    pub id: String,
    /// Short name (e.g., "blazing")
    pub name: String,
    /// Icon for display (e.g., "🔥")
    pub icon: String,
    /// Git hash of the binary
    pub git_hash: String,
    /// Version string (e.g., "v0.1.123")
    pub version: String,
}

impl ServerIdentity {
    /// Display name with icon (e.g., "🔥 blazing")
    pub fn display_name(&self) -> String {
        format!("{} {}", self.icon, self.name)
    }
}

pub(crate) fn startup_headless_recovery_test_delay() -> Option<std::time::Duration> {
    let raw = std::env::var("JCODE_TEST_HEADLESS_STARTUP_RECOVERY_DELAY_MS").ok()?;
    let delay_ms = raw.trim().parse::<u64>().ok()?;
    (delay_ms > 0).then(|| std::time::Duration::from_millis(delay_ms))
}

#[cfg(test)]
mod newer_binary_tests {
    use super::newer_binary_available;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn reports_update_when_candidate_is_strictly_newer() {
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(200)))];
        assert!(newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn ignores_candidate_that_is_not_newer() {
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn never_reloads_into_self_even_if_paths_were_equal() {
        // Same canonical path must never count as an update, regardless of mtime.
        let candidates = vec![(PathBuf::from("/x/current/jcode"), Some(t(999)))];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn suppresses_update_when_current_mtime_unavailable() {
        // Regression for issue #277: an unreadable current mtime previously fell
        // through to a path-difference heuristic that could loop forever.
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(200)))];
        assert!(!newer_binary_available(
            None,
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn suppresses_update_when_candidate_mtime_unavailable() {
        // The dangerous case from issue #277: candidate path differs but its
        // mtime cannot be read. Must NOT report an update.
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), None)];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn reports_update_if_any_candidate_is_newer() {
        let candidates = vec![
            (PathBuf::from("/x/stable/jcode"), None),
            (PathBuf::from("/x/shared/jcode"), Some(t(300))),
        ];
        assert!(newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/current/jcode")),
            candidates,
        ));
    }

    #[test]
    fn newer_server_is_not_outdated_by_older_channel_binary() {
        // Issue #291: a newer self-dev / shared-server daemon must NOT report an
        // update just because an *older* channel binary exists. Here the running
        // server (t=300) is newer than the only candidate (stable at t=100), so
        // there is no update. Previously a channel-version *mismatch* short-circuit
        // reported `true` here and told the newer server to downgrade itself.
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
        assert!(!newer_binary_available(
            Some(t(300)),
            Some(std::path::Path::new("/x/builds/versions/dev/jcode")),
            candidates,
        ));
    }

    #[test]
    fn equal_mtime_channel_binary_is_not_an_update() {
        // A candidate with the same mtime is not strictly newer, so it must not
        // trigger a reload (avoids the differ-but-not-newer reload loop, #277).
        let candidates = vec![(PathBuf::from("/x/stable/jcode"), Some(t(100)))];
        assert!(!newer_binary_available(
            Some(t(100)),
            Some(std::path::Path::new("/x/builds/versions/dev/jcode")),
            candidates,
        ));
    }
}

#[cfg(test)]
mod reload_target_tests {
    use super::{ReloadTargetDecision, guarded_reload_target};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn candidate(path: &str) -> (PathBuf, &'static str) {
        (PathBuf::from(path), "shared-server")
    }

    #[test]
    fn same_binary_is_always_used() {
        // Reloading into ourselves never raises a version question, even with an
        // older mtime reading.
        let decision = guarded_reload_target(
            candidate("/x/current/jcode"),
            Path::new("/x/current/jcode"),
            Some(Path::new("/x/current/jcode")),
            Some(Path::new("/x/current/jcode")),
            Some(t(200)),
            Some(t(100)),
        );
        assert!(matches!(decision, ReloadTargetDecision::UseCandidate(_)));
    }

    #[test]
    fn newer_candidate_is_used() {
        // The self-dev case: a freshly written candidate is newer, so apply it.
        let decision = guarded_reload_target(
            candidate("/x/shared-server/jcode"),
            Path::new("/x/builds/versions/new/jcode"),
            Some(Path::new("/x/builds/versions/old/jcode")),
            Some(Path::new("/x/builds/versions/old/jcode")),
            Some(t(100)),
            Some(t(200)),
        );
        match decision {
            ReloadTargetDecision::UseCandidate((path, _)) => {
                assert_eq!(path, PathBuf::from("/x/shared-server/jcode"));
            }
            other => panic!("expected candidate to be used, got {other:?}"),
        }
    }

    #[test]
    fn equal_mtime_candidate_is_used() {
        // Same mtime is not a downgrade.
        let decision = guarded_reload_target(
            candidate("/x/shared-server/jcode"),
            Path::new("/x/builds/versions/same/jcode"),
            Some(Path::new("/x/builds/versions/current/jcode")),
            Some(Path::new("/x/builds/versions/current/jcode")),
            Some(t(100)),
            Some(t(100)),
        );
        assert!(matches!(decision, ReloadTargetDecision::UseCandidate(_)));
    }

    #[test]
    fn strictly_older_candidate_is_blocked_and_uses_current_exe() {
        // The reported bug: shared-server channel points at an older build than
        // the running client. Force reload must NOT downgrade; it re-execs the
        // current binary instead.
        let decision = guarded_reload_target(
            candidate("/x/shared-server/jcode"),
            Path::new("/x/builds/versions/old-0.14.3/jcode"),
            Some(Path::new("/x/builds/versions/new/jcode")),
            Some(Path::new("/x/builds/versions/new/jcode")),
            Some(t(300)),
            Some(t(100)),
        );
        match decision {
            ReloadTargetDecision::DowngradeBlockedUseCurrent((path, _)) => {
                assert_eq!(path, PathBuf::from("/x/builds/versions/new/jcode"));
            }
            other => panic!("expected downgrade to be blocked, got {other:?}"),
        }
    }

    #[test]
    fn unreadable_candidate_mtime_is_treated_as_downgrade() {
        let decision = guarded_reload_target(
            candidate("/x/shared-server/jcode"),
            Path::new("/x/builds/versions/unknown/jcode"),
            Some(Path::new("/x/builds/versions/new/jcode")),
            Some(Path::new("/x/builds/versions/new/jcode")),
            Some(t(300)),
            None,
        );
        assert!(matches!(
            decision,
            ReloadTargetDecision::DowngradeBlockedUseCurrent(_)
        ));
    }

    #[test]
    fn downgrade_without_current_exe_falls_back_to_candidate() {
        // If we cannot identify the running exe we cannot re-exec it, so we have
        // to proceed with the candidate rather than refuse to reload entirely.
        let decision = guarded_reload_target(
            candidate("/x/shared-server/jcode"),
            Path::new("/x/builds/versions/old/jcode"),
            None,
            None,
            None,
            Some(t(100)),
        );
        assert!(matches!(
            decision,
            ReloadTargetDecision::DowngradeUnverifiable(_)
        ));
    }
}

#[cfg(test)]
mod pick_newest_candidate_tests {
    use super::pick_newest_candidate;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn entry(
        path: &str,
        label: &'static str,
        mtime: Option<SystemTime>,
    ) -> ((PathBuf, &'static str), PathBuf, Option<SystemTime>) {
        let p = PathBuf::from(path);
        ((p.clone(), label), p, mtime)
    }

    #[test]
    fn other_flavor_wins_when_strictly_newer() {
        // The /update bug: the session's own (self-dev) flavor is pinned to an
        // OLD build, but the other (normal) flavor self-healed to a NEWER
        // release. The reload target must follow the newer release so the daemon
        // can actually apply the update it advertises.
        let chosen = pick_newest_candidate([
            entry(
                "/x/versions/old-selfdev/jcode",
                "shared-server",
                Some(t(100)),
            ),
            entry("/x/versions/new-release/jcode", "stable", Some(t(200))),
        ])
        .expect("a candidate");
        assert_eq!(chosen.0, PathBuf::from("/x/versions/new-release/jcode"));
    }

    #[test]
    fn own_flavor_wins_on_tie() {
        // A deliberately-pinned self-dev build that is at least as fresh as the
        // other flavor must be preserved (self-dev pin protection).
        let chosen = pick_newest_candidate([
            entry("/x/versions/selfdev/jcode", "shared-server", Some(t(200))),
            entry("/x/versions/release/jcode", "stable", Some(t(200))),
        ])
        .expect("a candidate");
        assert_eq!(chosen.0, PathBuf::from("/x/versions/selfdev/jcode"));
    }

    #[test]
    fn own_flavor_wins_when_strictly_newer() {
        let chosen = pick_newest_candidate([
            entry(
                "/x/versions/fresh-selfdev/jcode",
                "shared-server",
                Some(t(300)),
            ),
            entry("/x/versions/release/jcode", "stable", Some(t(200))),
        ])
        .expect("a candidate");
        assert_eq!(chosen.0, PathBuf::from("/x/versions/fresh-selfdev/jcode"));
    }

    #[test]
    fn unknown_other_mtime_never_displaces_preferred() {
        // An unreadable mtime on the other flavor must not let it win, so we
        // never swap to an unverifiable binary.
        let chosen = pick_newest_candidate([
            entry("/x/versions/selfdev/jcode", "shared-server", Some(t(100))),
            entry("/x/versions/release/jcode", "stable", None),
        ])
        .expect("a candidate");
        assert_eq!(chosen.0, PathBuf::from("/x/versions/selfdev/jcode"));
    }

    #[test]
    fn duplicate_canonical_paths_collapse() {
        // Both flavors resolving to the same binary must not double-count; the
        // first (preferred) occurrence wins.
        let chosen = pick_newest_candidate([
            entry("/x/versions/same/jcode", "shared-server", Some(t(100))),
            entry("/x/versions/same/jcode", "stable", Some(t(999))),
        ])
        .expect("a candidate");
        assert_eq!(chosen.1, "shared-server");
    }

    #[test]
    fn empty_is_none() {
        assert!(pick_newest_candidate(std::iter::empty()).is_none());
    }
}

#[cfg(test)]
mod newest_reload_candidate_integration_tests {
    //! End-to-end-ish coverage that drives `newest_reload_candidate` through the
    //! REAL channel resolution (`build::shared_server_update_candidate`) against
    //! a temp `JCODE_HOME`. This reproduces the field "/update -> new client,
    //! stale server" state and proves the fix: a self-dev daemon now reloads into
    //! the freshly installed release instead of its old pinned binary.
    use super::{
        canonicalize_or, comparison_path, newer_binary_available, newest_reload_candidate,
    };
    use crate::build;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn install_versioned_binary(version: &str, mtime: SystemTime) -> std::path::PathBuf {
        // A real, distinct file per version so mtimes are independently settable
        // (install hard-links the source, which would share an inode/mtime).
        let dir = build::builds_dir()
            .expect("builds dir")
            .join("versions")
            .join(version);
        std::fs::create_dir_all(&dir).expect("create version dir");
        let path = dir.join(build::binary_name());
        std::fs::write(&path, format!("binary for {version}")).expect("write binary");
        std::fs::File::open(&path)
            .expect("open binary")
            .set_modified(mtime)
            .expect("set mtime");
        path
    }

    /// Model a Linux compat install: `versions/<ver>/` holds the real
    /// `jcode-linux-x86_64.bin` plus the sh wrapper at `binary_name()` that
    /// exec's it. The wrapper is written (and therefore stamped) *after* the
    /// binary, exactly like the installer does. Returns the `.bin` path —
    /// what the daemon actually runs as.
    fn install_wrapped_versioned_binary(
        version: &str,
        bin_mtime: SystemTime,
    ) -> std::path::PathBuf {
        let dir = build::builds_dir()
            .expect("builds dir")
            .join("versions")
            .join(version);
        std::fs::create_dir_all(&dir).expect("create version dir");

        let bin = dir.join(super::wrapper_fixture::PLATFORM_BIN_NAME);
        std::fs::write(&bin, format!("platform binary for {version}")).expect("write .bin");
        std::fs::File::open(&bin)
            .expect("open .bin")
            .set_modified(bin_mtime)
            .expect("set .bin mtime");

        let wrapper = dir.join(build::binary_name());
        std::fs::write(&wrapper, super::wrapper_fixture::wrapper_script()).expect("write wrapper");
        std::fs::File::open(&wrapper)
            .expect("open wrapper")
            .set_modified(bin_mtime + Duration::from_millis(5))
            .expect("set wrapper mtime");

        bin
    }

    fn candidate_version_for(is_selfdev: bool) -> Option<String> {
        let (path, _label) = newest_reload_candidate(is_selfdev)?;
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        canonical
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
    }

    #[test]
    fn selfdev_daemon_reloads_into_fresh_release_after_update() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // Field state: shared-server pinned to an OLD self-dev build; stable
        // lags. Then `/update` installs a NEWER release and advances
        // stable/current (but NOT the pinned shared-server channel).
        let old_selfdev = "3f160da1-dirty-e756d52efca9";
        let new_release = "0.15.0";
        install_versioned_binary(old_selfdev, base);
        install_versioned_binary(new_release, base + Duration::from_secs(60));

        build::update_shared_server_symlink(old_selfdev).expect("pin shared-server");
        build::update_stable_symlink(new_release).expect("stable advanced by update");
        build::update_current_symlink(new_release).expect("current advanced by update");

        // The self-dev session's reload target must now be the fresh release, not
        // the stale pinned build. This is the fix.
        assert_eq!(
            candidate_version_for(true).as_deref(),
            Some(new_release),
            "self-dev daemon should reload into the freshly installed release"
        );
        // The normal session is unaffected (already healed to stable/release).
        assert_eq!(
            candidate_version_for(false).as_deref(),
            Some(new_release),
            "normal daemon should also target the fresh release"
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn selfdev_pin_is_preserved_when_it_is_the_freshest_build() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // A deliberately-promoted self-dev build that is NEWER than stable must
        // still be honored: the whole point of pinning shared-server.
        let stable_old = "0.14.3";
        let selfdev_new = "56f43c3d-dirty-deadbeef";
        install_versioned_binary(stable_old, base);
        install_versioned_binary(selfdev_new, base + Duration::from_secs(120));

        build::update_stable_symlink(stable_old).expect("stable");
        build::update_shared_server_symlink(selfdev_new).expect("pin newer self-dev");

        assert_eq!(
            candidate_version_for(true).as_deref(),
            Some(selfdev_new),
            "a fresher self-dev pin must be preserved for self-dev sessions"
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    /// Re-implements `server_has_newer_binary`'s decision against an *injected*
    /// running-daemon path + mtime, so a test can model "the daemon is still the
    /// OLD binary" without spawning a real process. It scans the exact same
    /// candidate set (both flavors) and uses the same `newer_binary_available`
    /// core the production function uses.
    fn daemon_reports_update(running: &Path, running_mtime: SystemTime) -> bool {
        let running_canonical = comparison_path(canonicalize_or(running.to_path_buf()));
        let mut candidates = std::collections::HashSet::new();
        for is_selfdev in [false, true] {
            if let Some((candidate, _label)) = super::server_update_candidate(is_selfdev) {
                candidates.insert(comparison_path(canonicalize_or(candidate)));
            }
        }
        let with_mtimes = candidates.into_iter().map(|candidate| {
            let m = std::fs::metadata(&candidate)
                .ok()
                .and_then(|m| m.modified().ok());
            (candidate, m)
        });
        newer_binary_available(
            Some(running_mtime),
            Some(running_canonical.as_path()),
            with_mtimes,
        )
    }

    /// The question that matters for shipped users: after a NORMAL (non-self-dev)
    /// `/update`, does the long-lived daemon actually advertise + apply the
    /// upgrade on reconnect?
    ///
    /// Models a normal install: `shared-server` was tracking `stable`, the daemon
    /// is running the old release, and `/update` installs a newer release and
    /// advances stable/current/shared-server. We then drive the REAL
    /// update-detection core and reload-target resolver and assert both:
    /// (1) the daemon reports `server_has_update = true`, and
    /// (2) the binary it reloads into is the freshly installed release.
    #[test]
    fn normal_user_daemon_detects_and_targets_update_after_update() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let old_release = "0.14.3";
        let new_release = "0.15.0";
        let old_path = install_versioned_binary(old_release, base);
        install_versioned_binary(new_release, base + Duration::from_secs(60));

        // Pre-update state: every channel on the old release (shared-server
        // tracking stable). This is the steady state for a normal user.
        build::update_stable_symlink(old_release).expect("stable old");
        build::update_current_symlink(old_release).expect("current old");
        build::update_shared_server_symlink(old_release).expect("shared old");

        // `/update` installs the new release and advances the channels. Because
        // shared-server was tracking stable, it advances too.
        build::advance_shared_server_if_tracking_stable(new_release).expect("advance shared");
        build::update_stable_symlink(new_release).expect("stable new");
        build::update_current_symlink(new_release).expect("current new");

        // (1) The daemon (still the OLD binary) must now SEE the update so it
        // reports server_has_update = true to reconnecting clients.
        assert!(
            daemon_reports_update(&old_path, base),
            "normal-user daemon should report a server update after /update advanced the channels"
        );

        // (2) The binary it reloads into must be the freshly installed release.
        assert_eq!(
            candidate_version_for(false).as_deref(),
            Some(new_release),
            "normal-user daemon should reload into the freshly installed release"
        );
    }

    /// Regression: a Linux compat install (wrapper + `.bin`) must NOT report an
    /// update against itself. The installer stamps the wrapper a few ms after
    /// the `.bin`, the channel symlinks point at the wrapper, and the daemon
    /// runs the `.bin` — comparing wrapper mtime vs `.bin` mtime made
    /// `server_has_update` stick to `true` forever, so every client demanded a
    /// reload that changed nothing (reload loop).
    #[test]
    fn wrapped_install_does_not_report_phantom_update_against_itself() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let release = "0.25.1";
        let running_bin = install_wrapped_versioned_binary(release, base);

        build::update_stable_symlink(release).expect("stable");
        build::update_current_symlink(release).expect("current");
        build::update_shared_server_symlink(release).expect("shared");

        assert!(
            !daemon_reports_update(&running_bin, base),
            "daemon running the .bin of the installed release must not see the \
             sibling wrapper as a newer binary"
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    /// A genuine update must still be detected when both versions are wrapped
    /// installs, and the reload must target the new version.
    #[test]
    fn wrapped_install_still_detects_real_update() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let old_release = "0.25.0";
        let new_release = "0.25.1";
        let old_bin = install_wrapped_versioned_binary(old_release, base);
        install_wrapped_versioned_binary(new_release, base + Duration::from_secs(60));

        build::update_stable_symlink(old_release).expect("stable old");
        build::update_shared_server_symlink(old_release).expect("shared old");
        build::advance_shared_server_if_tracking_stable(new_release).expect("advance shared");
        build::update_stable_symlink(new_release).expect("stable new");
        build::update_current_symlink(new_release).expect("current new");

        assert!(
            daemon_reports_update(&old_bin, base),
            "daemon running the old .bin must see the newly installed wrapped release"
        );
        assert_eq!(
            candidate_version_for(false).as_deref(),
            Some(new_release),
            "reload must target the freshly installed wrapped release"
        );

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

/// Shared fixture for the launcher-wrapper layout (`jcode` sh script next to
/// the platform `.bin` it exec's), so the unit and integration tests below
/// cannot drift apart on the wrapper shape they model.
#[cfg(test)]
pub(crate) mod wrapper_fixture {
    pub(crate) const PLATFORM_BIN_NAME: &str = "jcode-linux-x86_64.bin";

    pub(crate) fn wrapper_script() -> String {
        format!(
            "#!/usr/bin/env sh\nset -eu\nself_dir=$(dirname -- \"$0\")\nexec \"$self_dir/{PLATFORM_BIN_NAME}\" \"$@\"\n"
        )
    }
}

#[cfg(test)]
mod launcher_wrapper_tests {
    use super::wrapper_fixture::{PLATFORM_BIN_NAME, wrapper_script};
    use super::{comparison_path, launcher_wrapper_target};

    #[test]
    fn resolves_wrapper_to_sibling_bin() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let bin = dir.path().join(PLATFORM_BIN_NAME);
        std::fs::write(&bin, "elf bytes").expect("write bin");
        let wrapper = dir.path().join("jcode");
        std::fs::write(&wrapper, wrapper_script()).expect("write wrapper");

        assert_eq!(launcher_wrapper_target(&wrapper), Some(bin.clone()));
        assert_eq!(comparison_path(wrapper), bin);
    }

    #[test]
    fn falls_back_to_unique_sibling_bin_when_exec_line_is_unrecognized() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let bin = dir.path().join("jcode-linux-aarch64.bin");
        std::fs::write(&bin, "elf bytes").expect("write bin");
        let wrapper = dir.path().join("jcode");
        std::fs::write(&wrapper, "#!/bin/sh\n# reshaped wrapper\nrun_it\n").expect("write wrapper");

        assert_eq!(launcher_wrapper_target(&wrapper), Some(bin));
    }

    #[test]
    fn leaves_real_binaries_alone() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Non-script content: not a wrapper even though it is small.
        let exe = dir.path().join("jcode");
        std::fs::write(&exe, "\x7fELF...").expect("write exe");
        assert_eq!(launcher_wrapper_target(&exe), None);
        assert_eq!(comparison_path(exe.clone()), exe);
    }

    #[test]
    fn ignores_script_without_any_bin_sibling() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let wrapper = dir.path().join("jcode");
        std::fs::write(&wrapper, wrapper_script()).expect("write wrapper");
        // exec target missing and no sibling .bin: treat as a plain file.
        assert_eq!(launcher_wrapper_target(&wrapper), None);
    }

    #[test]
    fn refuses_ambiguous_sibling_fallback() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join(PLATFORM_BIN_NAME), "a").expect("write bin a");
        std::fs::write(dir.path().join("jcode-linux-aarch64.bin"), "b").expect("write bin b");
        let wrapper = dir.path().join("jcode");
        std::fs::write(&wrapper, "#!/bin/sh\nrun_it\n").expect("write wrapper");
        assert_eq!(launcher_wrapper_target(&wrapper), None);
    }
}

#[cfg(test)]
mod deleted_suffix_tests {
    use super::strip_deleted_suffix;
    use std::path::PathBuf;

    #[test]
    fn strips_linux_deleted_marker() {
        let p = PathBuf::from("/home/u/.jcode/builds/versions/abc/jcode (deleted)");
        assert_eq!(
            strip_deleted_suffix(p),
            PathBuf::from("/home/u/.jcode/builds/versions/abc/jcode")
        );
    }

    #[test]
    fn leaves_normal_paths_untouched() {
        let p = PathBuf::from("/home/u/.jcode/builds/versions/abc/jcode");
        assert_eq!(strip_deleted_suffix(p.clone()), p);
    }

    #[test]
    fn only_strips_trailing_marker() {
        // A path that merely contains the substring must not be altered.
        let p = PathBuf::from("/home/u/jcode (deleted)/jcode");
        assert_eq!(strip_deleted_suffix(p.clone()), p);
    }
}
