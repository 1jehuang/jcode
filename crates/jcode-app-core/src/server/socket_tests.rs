#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::reload_state::{ForeignMarkerPolicy, publish_reload_socket_ready};
use super::socket::sibling_socket_path;
#[cfg(unix)]
use super::socket::{
    daemon_lock_path_for, server_start_matches_existing_server, try_acquire_daemon_lock,
};
use super::{
    ReloadPhase, ReloadState, ReloadWaitStatus, await_reload_handoff, cleanup_socket_pair,
    clear_reload_marker, inspect_reload_wait_status, reload_marker_active, reload_marker_path,
    reload_process_alive, write_reload_state,
};
#[cfg(unix)]
use super::{connect_socket, reap_stale_socket_if_dead};
#[cfg(unix)]
use crate::transport::Listener;
use std::time::Duration;

#[test]
fn sibling_socket_path_roundtrip() {
    let main = std::path::PathBuf::from("/tmp/jcode.sock");
    let debug = std::path::PathBuf::from("/tmp/jcode-debug.sock");

    assert_eq!(sibling_socket_path(&main), Some(debug.clone()));
    assert_eq!(sibling_socket_path(&debug), Some(main));
}

#[test]
fn cleanup_socket_pair_removes_main_and_debug_files() {
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let dir = std::env::temp_dir();
    let main = dir.join(format!("jcode-test-{}.sock", stamp));
    let debug = dir.join(format!("jcode-test-{}-debug.sock", stamp));

    std::fs::write(&main, b"").expect("create main socket placeholder");
    std::fs::write(&debug, b"").expect("create debug socket placeholder");

    cleanup_socket_pair(&main);

    assert!(!main.exists(), "main socket file should be removed");
    assert!(!debug.exists(), "debug socket file should be removed");
}

#[cfg(unix)]
#[tokio::test]
async fn connect_socket_preserves_refused_socket_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("jcode.sock");

    {
        let _listener = Listener::bind(&socket_path).expect("bind listener");
    }

    assert!(
        socket_path.exists(),
        "listener drop should leave the socket path behind for stale-socket checks"
    );

    let err = connect_socket(&socket_path)
        .await
        .expect_err("connect should fail once the listener is gone");
    assert!(
        err.to_string().contains("refused the connection"),
        "unexpected error: {err:#}"
    );
    assert!(
        socket_path.exists(),
        "connect_socket should not unlink the socket path on connection refusal"
    );
}

#[cfg(unix)]
#[test]
fn daemon_lock_serializes_server_processes() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let lock_path = daemon_lock_path_for(&temp.path().join("jcode.sock"));
    let first = try_acquire_daemon_lock(&lock_path)
        .expect("acquire first daemon lock")
        .expect("first daemon lock should succeed");
    let second = try_acquire_daemon_lock(&lock_path).expect("acquire second daemon lock");
    assert!(second.is_none(), "second daemon lock should fail");
    drop(first);

    let third = try_acquire_daemon_lock(&lock_path)
        .expect("acquire third daemon lock")
        .expect("third daemon lock should succeed after release");
    drop(third);

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn reap_stale_socket_removes_dead_socket_pair_and_lock() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let socket = temp.path().join("jcode.sock");
    let debug = temp.path().join("jcode-debug.sock");
    let lock = daemon_lock_path_for(&socket);

    // Simulate the post-upgrade/crash state: socket + debug + lock files left
    // behind, but no process is listening on the socket.
    std::fs::write(&socket, b"").expect("write stale socket");
    std::fs::write(&debug, b"").expect("write stale debug socket");
    std::fs::write(&lock, b"").expect("write stale lock");

    let reaped = reap_stale_socket_if_dead(&socket).await;
    assert!(reaped, "a dead socket with no listener should be reaped");
    assert!(!socket.exists(), "stale socket should be removed");
    assert!(!debug.exists(), "stale debug socket should be removed");
    assert!(!lock.exists(), "stale daemon lock should be removed");

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn reap_stale_socket_spares_live_listener() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let socket = temp.path().join("jcode.sock");
    // A live listener means a daemon is bound; reaping must be a no-op.
    let listener = Listener::bind(&socket).expect("bind listener");

    let reaped = reap_stale_socket_if_dead(&socket).await;
    assert!(!reaped, "a live listener must never be reaped");
    assert!(socket.exists(), "live socket must be left intact");

    drop(listener);
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn reap_stale_socket_spares_socket_when_lock_is_held() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let socket = temp.path().join("jcode.sock");
    std::fs::write(&socket, b"").expect("write stale-looking socket");

    // Hold the daemon lock, emulating a live daemon whose socket probe happens
    // to be momentarily unanswerable. The reaper must not unlink the socket.
    let lock_path = daemon_lock_path_for(&socket);
    let held = try_acquire_daemon_lock(&lock_path)
        .expect("acquire lock")
        .expect("lock should be free");

    let reaped = reap_stale_socket_if_dead(&socket).await;
    assert!(
        !reaped,
        "socket must be spared while the daemon lock is held"
    );
    assert!(
        socket.exists(),
        "socket must be left intact while lock is held"
    );

    drop(held);
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn socket_override_is_reported_as_custom() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    let prev_socket = std::env::var_os("JCODE_SOCKET");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    crate::env::remove_var("JCODE_SOCKET");
    assert_eq!(super::socket_path(), super::default_socket_path());
    assert!(!super::socket_path_is_custom());

    // An override that resolves to the shared socket is not "custom": the
    // shared daemon owns that path, so nothing may start a second server there.
    crate::env::set_var("JCODE_SOCKET", temp.path().join("jcode.sock"));
    assert!(!super::socket_path_is_custom());

    crate::env::set_var("JCODE_SOCKET", temp.path().join("run-isolated.sock"));
    assert!(super::socket_path_is_custom());

    if let Some(prev_socket) = prev_socket {
        crate::env::set_var("JCODE_SOCKET", prev_socket);
    } else {
        crate::env::remove_var("JCODE_SOCKET");
    }
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

/// Spellings that resolve to the shared socket must not be treated as custom.
///
/// Comparing path spellings called a relative override or a symlinked parent
/// "custom", so `run` would start a *temporary* server on the shared socket:
/// other clients connect to it and then lose their server when the run exits
/// (review of #1768, finding 1). The daemon lock has to agree, or two daemons
/// could bind one socket under two spellings.
#[cfg(unix)]
#[test]
fn shared_socket_aliases_are_not_custom() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    let prev_socket = std::env::var_os("JCODE_SOCKET");
    let prev_cwd = std::env::current_dir().ok();

    // The runtime dir itself must be canonical: on macOS a tempdir lives under
    // /var -> /private/var, and the comparison resolves both sides anyway, but
    // keeping the baseline canonical makes the assertions below about *aliases*
    // rather than about the fixture.
    let runtime_dir = std::fs::canonicalize(temp.path()).expect("canonicalize tempdir");
    crate::env::set_var("JCODE_RUNTIME_DIR", &runtime_dir);
    let shared = runtime_dir.join("jcode.sock");

    // Alias 1: relative spelling, resolved against the current directory.
    std::env::set_current_dir(&runtime_dir).expect("cd into runtime dir");
    crate::env::set_var("JCODE_SOCKET", "jcode.sock");
    assert!(
        !super::socket_path_is_custom(),
        "a relative spelling of the shared socket must not be custom"
    );
    assert_eq!(
        daemon_lock_path_for(std::path::Path::new("jcode.sock")),
        runtime_dir.join("jcode-daemon.lock"),
        "a relative spelling must contend for the shared daemon lock"
    );

    // Alias 2: a `..` segment that cancels out.
    let dotted = runtime_dir.join("sub/../jcode.sock");
    std::fs::create_dir_all(runtime_dir.join("sub")).expect("create sub dir");
    crate::env::set_var("JCODE_SOCKET", &dotted);
    assert!(
        !super::socket_path_is_custom(),
        "a path with a cancelling .. segment must not be custom"
    );

    // Alias 3: symlinked parent directory pointing at the runtime dir.
    let link_parent = temp.path().join("link");
    std::os::unix::fs::symlink(&runtime_dir, &link_parent).expect("symlink runtime dir");
    let via_link = link_parent.join("jcode.sock");
    crate::env::set_var("JCODE_SOCKET", &via_link);
    assert!(
        !super::socket_path_is_custom(),
        "the shared socket reached through a symlinked parent must not be custom"
    );
    assert_eq!(
        daemon_lock_path_for(&via_link),
        runtime_dir.join("jcode-daemon.lock"),
        "a symlinked parent must contend for the shared daemon lock"
    );
    assert!(super::is_shared_socket(&via_link));

    // A genuinely different socket is still custom, and two spellings of it
    // share one lock.
    let isolated = runtime_dir.join("run-isolated.sock");
    crate::env::set_var("JCODE_SOCKET", &isolated);
    assert!(super::socket_path_is_custom());
    assert_eq!(
        daemon_lock_path_for(&link_parent.join("run-isolated.sock")),
        daemon_lock_path_for(&isolated),
        "two spellings of one custom socket must share a lock"
    );
    assert_ne!(
        daemon_lock_path_for(&isolated),
        daemon_lock_path_for(&shared)
    );

    if let Some(prev_cwd) = prev_cwd {
        let _ = std::env::set_current_dir(prev_cwd);
    }
    if let Some(prev_socket) = prev_socket {
        crate::env::set_var("JCODE_SOCKET", prev_socket);
    } else {
        crate::env::remove_var("JCODE_SOCKET");
    }
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

/// The final path component must be taken as written, never followed.
///
/// Binding unlinks and recreates that component, so a socket path that happens
/// to be a symlink to another socket ends up owning its own, distinct socket.
/// Resolving through the link made the server lock the *target's* file and then
/// publish a different socket, so a later server on the target path was
/// rejected with "already running" while nothing listened there (review of
/// #1768, finding 2).
/// A custom-socket server reloads itself too, and on Unix the exec keeps the
/// same pid, so it must still advance its *own* marker to `SocketReady`.
/// Suppressing the publish for custom sockets outright left the marker stuck
/// in `Starting`, and the reloaded server rejected new messages until it
/// expired (review of #1768, "Custom reloads keep clients waiting").
#[test]
fn own_reload_marker_is_published_even_when_foreign_markers_are_preserved() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let current = std::process::id();
    write_reload_state("req-own", "hash-own", ReloadPhase::Starting, None);

    publish_reload_socket_ready(ForeignMarkerPolicy::Preserve);

    let state = ReloadState::load().expect("own marker must survive the publish");
    assert_eq!(
        state.phase,
        ReloadPhase::SocketReady,
        "a server must advance its own marker regardless of the foreign-marker policy"
    );
    assert_eq!(state.pid, current);

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

/// The mirror case: a marker owned by a *different* live process is left
/// alone, so a custom-socket server cannot erase the shared daemon's in-flight
/// `Starting` state.
#[test]
fn foreign_reload_marker_is_preserved_under_the_preserve_policy() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let foreign_pid = std::process::id().wrapping_add(1).max(1);
    ReloadState {
        request_id: "req-foreign".to_string(),
        hash: "hash-foreign".to_string(),
        phase: ReloadPhase::Starting,
        pid: foreign_pid,
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();

    publish_reload_socket_ready(ForeignMarkerPolicy::Preserve);
    let preserved = ReloadState::load().expect("foreign marker must be preserved");
    assert_eq!(preserved.phase, ReloadPhase::Starting);
    assert_eq!(preserved.pid, foreign_pid);

    // And the shared daemon is still allowed to clean it up.
    publish_reload_socket_ready(ForeignMarkerPolicy::MayClear);
    assert!(
        ReloadState::load().is_none(),
        "the shared socket's server must still clear a stale foreign marker"
    );

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[cfg(unix)]
#[test]
fn daemon_lock_does_not_follow_the_final_socket_symlink() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    let runtime_dir = std::fs::canonicalize(temp.path()).expect("canonicalize tempdir");
    crate::env::set_var("JCODE_RUNTIME_DIR", &runtime_dir);

    // A stale socket, plus an alias path symlinked onto it.
    let target = runtime_dir.join("target.sock");
    std::fs::write(&target, b"").expect("write stale target socket");
    let alias = runtime_dir.join("alias.sock");
    std::os::unix::fs::symlink(&target, &alias).expect("symlink alias onto target");

    assert_ne!(
        daemon_lock_path_for(&alias),
        daemon_lock_path_for(&target),
        "the alias must not borrow the target's lock"
    );
    assert_eq!(
        daemon_lock_path_for(&alias),
        runtime_dir.join("alias.sock.daemon.lock")
    );

    // Both are custom, and each keeps its own lock, so a server on the target
    // path is not blocked by a server on the alias path.
    let alias_held = try_acquire_daemon_lock(&daemon_lock_path_for(&alias))
        .expect("acquire alias lock")
        .expect("alias lock should be free");
    let target_held = try_acquire_daemon_lock(&daemon_lock_path_for(&target))
        .expect("acquire target lock")
        .expect("target lock must be free while the alias lock is held");

    drop(target_held);
    drop(alias_held);
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

/// A custom `--socket` must get its own daemon lock. With a single
/// runtime-dir-wide lock, a second server could never start, which is why
/// `jcode run --socket X` had no way to ever get a listener on `X` (#1748).
#[cfg(unix)]
#[test]
fn daemon_lock_is_socket_scoped_for_custom_sockets() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    // Canonical fixture: lock paths are derived from the resolved socket, and
    // on macOS a tempdir under /var resolves to /private/var.
    let runtime_dir = std::fs::canonicalize(temp.path()).expect("canonicalize tempdir");
    crate::env::set_var("JCODE_RUNTIME_DIR", &runtime_dir);

    let shared = runtime_dir.join("jcode.sock");
    let isolated = runtime_dir.join("run-isolated.sock");

    let shared_lock = daemon_lock_path_for(&shared);
    let isolated_lock = daemon_lock_path_for(&isolated);

    // The shared socket keeps the historical name so a daemon from an older
    // build still blocks a second shared daemon across an upgrade.
    assert_eq!(shared_lock, runtime_dir.join("jcode-daemon.lock"));
    assert_eq!(
        isolated_lock,
        runtime_dir.join("run-isolated.sock.daemon.lock")
    );

    // Both locks are holdable at once: the shared daemon keeps running while
    // an isolated server binds its own socket.
    let shared_held = try_acquire_daemon_lock(&shared_lock)
        .expect("acquire shared daemon lock")
        .expect("shared daemon lock should be free");
    let isolated_held = try_acquire_daemon_lock(&isolated_lock)
        .expect("acquire isolated daemon lock")
        .expect("isolated daemon lock should be free while the shared one is held");

    // Within one socket the lock is still exclusive.
    assert!(
        try_acquire_daemon_lock(&isolated_lock)
            .expect("re-acquire isolated daemon lock")
            .is_none(),
        "a second server on the same socket must still be rejected"
    );

    drop(isolated_held);
    drop(shared_held);
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[cfg(unix)]
#[test]
fn existing_server_start_errors_are_detected() {
    assert!(server_start_matches_existing_server(
        "Error: Another jcode server process is already running for runtime dir /run/user/1000"
    ));
    assert!(server_start_matches_existing_server(
        "Error: Another jcode server process is already running on socket /tmp/run.sock"
    ));
    assert!(server_start_matches_existing_server(
        "Error: Refusing to replace active server socket at /run/user/1000/jcode.sock"
    ));
    assert!(!server_start_matches_existing_server(
        "Error: failed to bind socket: permission denied"
    ));
}

#[test]
fn reload_marker_active_expires_stale_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let marker = reload_marker_path();
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_reload_state("test-request", "test-hash", ReloadPhase::Starting, None);
    assert!(reload_marker_active(Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(5));
    assert!(!reload_marker_active(Duration::ZERO));
    assert!(!marker.exists(), "stale reload marker should be cleaned up");

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn reload_marker_active_for_recent_socket_ready_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    write_reload_state("test-request", "test-hash", ReloadPhase::SocketReady, None);
    assert!(reload_marker_active(Duration::from_secs(30)));

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn publish_reload_socket_ready_updates_current_process_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    write_reload_state(
        "test-request",
        "test-hash",
        ReloadPhase::Starting,
        Some("detail".to_string()),
    );
    publish_reload_socket_ready(ForeignMarkerPolicy::MayClear);

    let state = ReloadState::load().expect("reload state should exist");
    assert_eq!(state.phase, ReloadPhase::SocketReady);
    assert_eq!(state.request_id, "test-request");
    assert_eq!(state.hash, "test-hash");
    assert_eq!(state.detail.as_deref(), Some("detail"));

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn publish_reload_socket_ready_clears_marker_for_foreign_pid() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    ReloadState {
        request_id: "test-request".to_string(),
        hash: "test-hash".to_string(),
        phase: ReloadPhase::Starting,
        pid: std::process::id().saturating_add(1_000_000),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();

    publish_reload_socket_ready(ForeignMarkerPolicy::MayClear);
    assert!(
        ReloadState::load().is_none(),
        "foreign reload marker should be cleared"
    );

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn inspect_reload_wait_status_reports_ready_for_socket_ready_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    write_reload_state("test-request", "test-hash", ReloadPhase::SocketReady, None);

    let socket_path = temp.path().join("missing.sock");
    let status = inspect_reload_wait_status(&socket_path, Duration::from_secs(30), None).await;
    assert_eq!(status, ReloadWaitStatus::Ready);

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn inspect_reload_wait_status_keeps_waiting_while_starting_marker_is_active_even_if_socket_is_live()
 {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    ReloadState {
        request_id: "test-request".to_string(),
        hash: "test-hash".to_string(),
        phase: ReloadPhase::Starting,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();

    let socket_path = temp.path().join("jcode.sock");
    let _listener = Listener::bind(&socket_path).expect("bind listener");

    let status = inspect_reload_wait_status(&socket_path, Duration::from_secs(30), None).await;
    assert_eq!(
        status,
        ReloadWaitStatus::Waiting {
            pid: Some(std::process::id())
        }
    );

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn wait_for_reload_handoff_event_returns_promptly_when_no_event_arrives() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let socket_path = temp.path().join("missing.sock");
    let started = std::time::Instant::now();
    crate::server::wait_for_reload_handoff_event(Some(std::process::id()), &socket_path).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "reload handoff event wait should be a bounded edge wait, not an indefinite block"
    );

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn inspect_reload_wait_status_reports_idle_without_marker_or_listener() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("missing.sock");

    let status = inspect_reload_wait_status(&socket_path, Duration::from_secs(30), None).await;
    assert_eq!(status, ReloadWaitStatus::Idle);
}

#[tokio::test]
async fn inspect_reload_wait_status_uses_last_known_pid_when_marker_missing() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("missing.sock");

    let status = inspect_reload_wait_status(
        &socket_path,
        Duration::from_secs(30),
        Some(std::process::id()),
    )
    .await;
    assert_eq!(
        status,
        ReloadWaitStatus::Waiting {
            pid: Some(std::process::id())
        }
    );
}

#[tokio::test]
async fn inspect_reload_wait_status_reports_failed_when_reload_pid_is_dead() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
    let dead_pid = std::process::id().saturating_add(1_000_000);
    assert!(
        !reload_process_alive(dead_pid),
        "test requires a definitely-dead pid"
    );

    ReloadState {
        request_id: "test-request".to_string(),
        hash: "test-hash".to_string(),
        phase: ReloadPhase::Starting,
        pid: dead_pid,
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();

    let socket_path = temp.path().join("missing.sock");
    let status = inspect_reload_wait_status(&socket_path, Duration::from_secs(30), None).await;
    assert!(matches!(status, ReloadWaitStatus::Failed(Some(_))));

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn await_reload_handoff_returns_ready_after_marker_transition() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    ReloadState {
        request_id: "test-request".to_string(),
        hash: "test-hash".to_string(),
        phase: ReloadPhase::Starting,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();

    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        write_reload_state("test-request", "test-hash", ReloadPhase::SocketReady, None);
    });

    let socket_path = temp.path().join("missing.sock");
    let status = tokio::time::timeout(
        Duration::from_secs(2),
        await_reload_handoff(&socket_path, Duration::from_secs(30)),
    )
    .await
    .expect("await reload handoff should finish");
    assert_eq!(status, ReloadWaitStatus::Ready);

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn await_reload_handoff_returns_failed_after_marker_transition() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    ReloadState {
        request_id: "test-request".to_string(),
        hash: "test-hash".to_string(),
        phase: ReloadPhase::Starting,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();

    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        write_reload_state(
            "test-request",
            "test-hash",
            ReloadPhase::Failed,
            Some("boom".to_string()),
        );
    });

    let socket_path = temp.path().join("missing.sock");
    let status = tokio::time::timeout(
        Duration::from_secs(2),
        await_reload_handoff(&socket_path, Duration::from_secs(30)),
    )
    .await
    .expect("await reload handoff should finish");
    assert_eq!(status, ReloadWaitStatus::Failed(Some("boom".to_string())));

    clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}
