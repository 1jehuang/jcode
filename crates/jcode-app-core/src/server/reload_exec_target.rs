//! Selection of the reload executable with downgrade protection.

use super::*;

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
    // A managed Jcode Cloud daemon runs from an immutable release directory and
    // its runtime is selected by the root-owned updater through one launcher
    // link. That selection is authoritative (a pin may deliberately downgrade),
    // so reload follows it instead of the self-dev/stable channels in ~/.jcode.
    if let Some(target) = managed_cloud_reload_target(running_server_binary().as_deref()) {
        crate::logging::info(&format!(
            "{MANAGED_CLOUD_LIVE_RELOAD_MARKER}: exec target {:?}",
            target.0
        ));
        return Some(target);
    }
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
    let current_exe = running_server_binary();

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
