//! End-to-end-ish coverage that drives `newest_reload_candidate` through the
//! REAL channel resolution (`build::shared_server_update_candidate`) against
//! a temp `JCODE_HOME`. This reproduces the field "/update -> new client,
//! stale server" state and proves the fix: a self-dev daemon now reloads into
//! the freshly installed release instead of its old pinned binary.
use super::{newer_binary_available, newest_reload_candidate};
use crate::build;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Backdate a freshly written file so candidate ordering is decidable.
///
/// The handle needs write access, not read: Windows `SetFileTime` requires
/// FILE_WRITE_ATTRIBUTES, so `File::open` fails there with
/// ERROR_ACCESS_DENIED ("Acesso negado."), while `futimens` on Unix needs
/// no access mode at all. Measured on Windows:
///
///   File::open(..).set_modified(..)      -> PermissionDenied (code 5)
///   OpenOptions::new().write(true)      -> ok, exact round-trip (0ns)
fn set_mtime(path: &Path, mtime: SystemTime) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for mtime")
        .set_modified(mtime)
        .expect("set mtime");
}

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
    set_mtime(&path, mtime);
    path
}

/// Which version the reload would target, as a human-checkable name.
///
/// Read the channel's own version marker rather than recovering the version
/// from the candidate path. Windows `atomic_symlink_swap` copies the
/// launcher instead of symlinking it (Windows keeps a loaded executable
/// open, so the entry cannot be replaced by a link), so on Windows the
/// channel path canonicalizes to `stable/jcode.exe` and the version is not
/// in the path at all. Measured:
///
///   symlink -> canon .../versions/0.15.0/jcode.exe  (parent "0.15.0")
///   copy    -> canon .../stable/jcode.exe         (parent "stable")
///
/// Every channel that `update_*_symlink` publishes also writes a version
/// marker, so that is the portable source of truth.
fn candidate_version_for(is_selfdev: bool) -> Option<String> {
    let (path, label) = newest_reload_candidate(is_selfdev)?;
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    // A wrapper resolves to its `<stem>-*.bin` payload, whose sibling
    // directory is the version. Keep that, so a channel that really is a
    // symlink still reports its version.
    let canonical = build::resolve_binary_payload(&canonical);
    if let Some(version) = canonical
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        && version != "stable"
        && version != "current"
        && version != "shared-server"
    {
        return Some(version);
    }
    let marker = match label {
        "stable" => build::stable_version_file(),
        "current" => build::current_version_file(),
        "shared-server" => build::shared_server_version_file(),
        _ => return None,
    }
    .ok()?;
    Some(std::fs::read_to_string(marker).ok()?.trim().to_string())
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
/// core the production function uses, including the wrapper->payload
/// resolution.
fn daemon_reports_update(running: &Path, running_mtime: SystemTime) -> bool {
    let running_canonical = build::resolve_binary_payload(running);
    let mut candidates = std::collections::HashSet::new();
    for is_selfdev in [false, true] {
        if let Some((candidate, _label)) = super::server_update_candidate(is_selfdev) {
            candidates.insert(build::resolve_binary_payload(&candidate));
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

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    }
}

/// Install a release-archive-style version dir: a tiny `jcode` wrapper
/// script plus the real `jcode-linux-x86_64.bin` payload, with independently
/// settable mtimes. This is exactly what `/update`'s tar.gz install path
/// produces on disk.
fn install_release_style_binary(
    version: &str,
    wrapper_mtime: SystemTime,
    payload_mtime: SystemTime,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = build::builds_dir()
        .expect("builds dir")
        .join("versions")
        .join(version);
    std::fs::create_dir_all(&dir).expect("create version dir");
    // `resolve_binary_payload` finds the payload as a sibling
    // `<stem>-*.bin` of the wrapper, so the name only has to match that
    // glob. Keeping it platform independent lets the same fixture assert
    // the same layout everywhere.
    let payload = dir.join(format!("{}-payload.bin", build::binary_stem()));
    std::fs::write(&payload, format!("payload for {version}")).expect("write payload");
    set_mtime(&payload, payload_mtime);
    let wrapper = dir.join(build::binary_name());
    std::fs::write(
        &wrapper,
        format!(
            "#!/usr/bin/env sh\nexec ./{}-payload.bin \"$@\"\n",
            build::binary_stem()
        ),
    )
    .expect("write wrapper");
    set_mtime(&wrapper, wrapper_mtime);
    (wrapper, payload)
}

/// Regression test for the post-`/update` infinite reload loop: release
/// archives install a wrapper script + `.bin` payload, and the install copy
/// loop can write the wrapper AFTER the payload. The running daemon's
/// `current_exe()` is the payload, while the channel candidate resolves to
/// the wrapper. Comparing wrapper-vs-payload mtimes made the freshly
/// updated daemon report "newer binary available" against ITS OWN install
/// forever -> the client force-reloaded the server in a loop and the
/// session never attached.
/// The wrapper-plus-`.bin` layout exists only in the Linux release
/// archive. `scripts/install.ps1` installs a plain `jcode.exe` into
/// `versions/<version>/` and copies it to `stable/jcode.exe`, so a Windows
/// channel has no wrapper and no sibling payload. Measured on Windows: the
/// channel wrapper carries the fixture's mtime (`std::fs::copy` does
/// preserve it, delta 0ns), but `resolve_binary_payload` finds no sibling
/// `<stem>-*.bin` beside the channel copy, so it returns the wrapper and the
/// wrapper-vs-payload comparison degenerates into wrapper-vs-payload from
/// two different directories.
///
/// That is a fixture artifact, not the production shape this test guards:
/// the regression it covers is the post-`/update` infinite reload loop, and
/// there is no wrapper for that loop to happen on Windows. Restricting the
/// test keeps the guarantee where the hazard exists rather than asserting a
/// layout the Windows installer never creates.
#[cfg(unix)]
#[test]
fn freshly_updated_release_daemon_reports_no_phantom_update() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    // Wrapper written strictly AFTER the payload (the bad copy order).
    let (wrapper, payload) =
        install_release_style_binary("0.25.1", base + Duration::from_secs(5), base);
    build::update_stable_symlink("0.25.1").expect("stable");
    build::update_current_symlink("0.25.1").expect("current");
    build::update_shared_server_symlink("0.25.1").expect("shared");

    // The daemon runs the payload; the candidate is the wrapper. Same
    // logical install -> no update must be reported.
    let payload_mtime = std::fs::metadata(&payload)
        .expect("payload metadata")
        .modified()
        .expect("payload mtime");
    assert!(
        !daemon_reports_update(&payload, payload_mtime),
        "a freshly updated daemon must not report an update against its own install"
    );
    // Sanity: the wrapper IS strictly newer than the payload on disk, so a
    // naive wrapper-vs-payload comparison would have reported a phantom
    // update (the bug this guards against).
    let wrapper_mtime = std::fs::metadata(&wrapper)
        .expect("wrapper metadata")
        .modified()
        .expect("wrapper mtime");
    assert!(wrapper_mtime > payload_mtime);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
