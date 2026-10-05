use crate::build;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Default embedding idle unload threshold. The local MiniLM runtime adds about
/// 150 MiB of resident memory after its first query, while loading it takes only
/// about 200 ms and memory retrieval runs off the interactive turn path. Keep it
/// warm for short bursts, but do not pin it through long quiet periods.
const EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS: u64 = 60;

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
    parse_embedding_idle_unload_secs(
        std::env::var("JCODE_EMBEDDING_IDLE_UNLOAD_SECS")
            .ok()
            .as_deref(),
    )
}

fn parse_embedding_idle_unload_secs(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS)
}

#[cfg(test)]
#[cfg(test)]
#[path = "util/embedding_idle_tests.rs"]
mod embedding_idle_tests;

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

    // Identity/mtime comparisons must look through release wrapper scripts to
    // the payload that actually runs (see `build::resolve_binary_payload`):
    // the running exe is the `.bin` payload while channel candidates are tiny
    // wrapper scripts, and comparing wrapper-vs-payload mtimes turned every
    // release install into a phantom "downgrade"/"update". The exec target
    // stays the original candidate path (the wrapper), which is what sets up
    // `LD_LIBRARY_PATH` correctly.
    let candidate_canonical = build::resolve_binary_payload(&candidate.0);
    let current_canonical = current_exe
        .as_ref()
        .map(|p| build::resolve_binary_payload(p));

    let current_mtime = current_canonical.as_deref().and_then(binary_mtime);
    let candidate_mtime = binary_mtime(candidate_canonical.as_path());

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
        // Compare payloads, not release wrapper scripts (whose mtimes carry no
        // version information). Dedup also happens on the payload so a wrapper
        // and its payload never count as two distinct candidates.
        let canonical = build::resolve_binary_payload(&candidate.0);
        let mtime = binary_mtime(canonical.as_path());
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

/// Canonicalize a path for project-identity comparison, falling back to the
/// literal path when it cannot be resolved (the directory can be deleted while
/// the session that owns it stays alive).
///
/// Shared by every subsystem that decides whether two working directories name
/// the same project. One helper keeps that decision from drifting: if resume and
/// resume-all each canonicalized separately they could disagree about a symlinked
/// checkout and the same session would count as in-scope in one path and
/// out-of-scope in the other.
pub(super) fn canonicalize_or(path: PathBuf) -> PathBuf {
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

/// Return the swarm identity for an independently-created root session.
///
/// Swarm plans are keyed by swarm id. Deriving that id from the working
/// directory made every session opened in one repository share one plan, even
/// when those sessions were unrelated. Root sessions therefore own a swarm by
/// default. `JCODE_SWARM_ID` remains an explicit opt-in to a shared swarm.
pub(crate) fn swarm_id_for_session(session_id: &str) -> Option<String> {
    if let Ok(sw_id) = std::env::var("JCODE_SWARM_ID") {
        let trimmed = sw_id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    default_swarm_id_for_session(session_id)
}

fn default_swarm_id_for_session(session_id: &str) -> Option<String> {
    if session_id.trim().is_empty() {
        None
    } else {
        Some(format!("session:{session_id}"))
    }
}

#[cfg(test)]
#[cfg(test)]
#[path = "util/swarm_identity_tests.rs"]
mod swarm_identity_tests;

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
    //
    // All paths are resolved through `build::resolve_binary_payload` so release
    // installs (channel symlink -> wrapper script -> `.bin` payload) compare the
    // payload that actually runs. Comparing the wrapper script against the
    // running payload compared two different files with unrelated mtimes, which
    // could report a phantom update forever and wedge clients into an infinite
    // reload loop right after `/update`.
    let current_exe = std::env::current_exe().ok().map(strip_deleted_suffix);
    let current_canonical = current_exe
        .as_ref()
        .map(|path| build::resolve_binary_payload(path));
    let current_mtime = current_canonical
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());

    let mut candidates = HashSet::new();
    for is_selfdev_session in [false, true] {
        if let Some((candidate, _label)) = server_update_candidate(is_selfdev_session) {
            candidates.insert(build::resolve_binary_payload(&candidate));
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
#[cfg(test)]
#[path = "util/newer_binary_tests.rs"]
mod newer_binary_tests;

#[cfg(test)]
#[cfg(test)]
#[path = "util/reload_target_tests.rs"]
mod reload_target_tests;

#[cfg(test)]
#[cfg(test)]
#[path = "util/pick_newest_candidate_tests.rs"]
mod pick_newest_candidate_tests;

#[cfg(test)]
#[cfg(test)]
#[path = "util/newest_reload_candidate_integration_tests.rs"]
mod newest_reload_candidate_integration_tests;

#[cfg(test)]
#[cfg(test)]
#[path = "util/deleted_suffix_tests.rs"]
mod deleted_suffix_tests;
