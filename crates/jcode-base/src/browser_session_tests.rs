use super::*;
use std::os::unix::net::UnixListener;

#[test]
fn session_child_drops_only_verified_gate_descriptor() {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let gate = tempfile::NamedTempFile::new().unwrap();
    let unrelated = tempfile::NamedTempFile::new().unwrap();
    for (expected_gate, should_be_closed) in [(gate.path(), true), (unrelated.path(), false)] {
        let fd = gate.as_file().as_raw_fd();
        let mut command = std::process::Command::new("/bin/sh");
        command.args([
            "-c",
            if should_be_closed {
                "test ! -e /dev/fd/9"
            } else {
                "test -e /dev/fd/9"
            },
        ]);
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(fd, 9) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        prevent_inherited_cargo_gate(&mut command, Some(expected_gate));
        assert!(command.status().unwrap().success());
    }
    assert!(
        gate.as_file().metadata().is_ok(),
        "parent descriptor must stay open"
    );
}

#[test]
fn session_child_cannot_retain_parent_gate_lock() {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let gate = tempfile::NamedTempFile::new().unwrap();
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(gate.path())
        .unwrap();
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(gate.path())
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let owner_fd = owner.as_raw_fd();
    let other_fds = [gate.as_file().as_raw_fd(), contender.as_raw_fd()];
    let mut command = std::process::Command::new("/bin/sleep");
    command.arg("1");
    unsafe {
        command.pre_exec(move || {
            for fd in other_fds.into_iter().chain(std::iter::once(owner_fd)) {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::dup2(owner_fd, 9) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    prevent_inherited_cargo_gate(&mut command, Some(gate.path()));
    let mut child = command.spawn().unwrap();
    assert_ne!(
        unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "parent must retain its lock"
    );
    let path = gate.path().to_path_buf();
    drop(contender);
    // Darwin flock lifetime can include other open handles to this vnode in
    // the owner process. Close all of them before checking the child alone.
    let (_, persisted_path) = gate.keep().unwrap();
    drop(owner);
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert!(child.try_wait().unwrap().is_none());
    assert_eq!(
        unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0,
        "child must not retain the gate after owner exits"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    drop(contender);
    std::fs::remove_file(persisted_path).unwrap();
}

#[test]
fn long_session_ids_bind_real_unix_sockets_for_all_bridge_browsers() {
    let _guard = crate::storage::lock_test_env();
    let previous_runtime = std::env::var_os("XDG_RUNTIME_DIR");
    crate::env::set_var("XDG_RUNTIME_DIR", "/tmp");
    for length in [49, 100, 300] {
        let id = format!("{}{}", "x".repeat(length), std::process::id());
        for kind in [
            BrowserKind::Chrome,
            BrowserKind::Firefox,
            BrowserKind::Chromium,
            BrowserKind::Brave,
            BrowserKind::Opera,
            BrowserKind::Edge,
        ] {
            let name = session_name_for(&id, Some(kind.id()));
            let path = session_socket_path(&name);
            use std::os::unix::ffi::OsStrExt;
            assert!(
                path.as_os_str().as_bytes().len() < 104,
                "{}",
                path.display()
            );
            let listener = UnixListener::bind(&path).unwrap();
            drop(listener);
            std::fs::remove_file(path).unwrap();
        }
    }
    let temp = tempfile::Builder::new()
        .prefix("jcode-socket-")
        .tempdir_in("/tmp")
        .unwrap();
    let longer_runtime = temp.path().join("r".repeat(30));
    std::fs::create_dir(&longer_runtime).unwrap();
    crate::env::set_var("XDG_RUNTIME_DIR", &longer_runtime);
    for browser in ["chrome", "firefox", "chromium", "brave", "opera", "edge"] {
        let name = session_name_for(&"long-session".repeat(20), Some(browser));
        let path = session_socket_path(&name);
        let listener = UnixListener::bind(&path).unwrap();
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }
    crate::env::set_var("XDG_RUNTIME_DIR", format!("/tmp/{}", "r".repeat(120)));
    assert!(ensure_browser_session_for("cannot-bind", Some("chrome")).is_none());
    match previous_runtime {
        Some(runtime) => crate::env::set_var("XDG_RUNTIME_DIR", runtime),
        None => crate::env::remove_var("XDG_RUNTIME_DIR"),
    }
}

#[test]
fn session_names_do_not_collide_when_sanitized_or_truncated() {
    let _guard = crate::storage::lock_test_env();
    assert_ne!(
        session_name_for("a/b", Some("chrome")),
        session_name_for("ab", Some("chrome"))
    );
    let common = "x".repeat(70);
    assert_ne!(
        session_name_for(&format!("{common}a"), Some("chrome")),
        session_name_for(&format!("{common}b"), Some("chrome"))
    );
    assert_ne!(
        session_name_for("same", Some("chrome")),
        session_name_for("same", Some("brave"))
    );
    assert!(session_name_for("same", Some("chrome")).ends_with("-target-v4"));
}

#[test]
fn unsupported_explicit_session_targets_do_not_fall_back() {
    assert!(ensure_browser_session_for("unsupported-test", Some("unknown-browser")).is_none());
    assert!(ensure_browser_session_for("unsupported-test", Some("safari")).is_none());
}

#[test]
fn browser_session_liveness_checks_pid_and_listener() {
    let _guard = crate::storage::lock_test_env();
    let name = format!("jcode-liveness-test-{}", std::process::id());
    let socket = session_socket_path(&name);
    let pid = session_pid_path(&name);
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::write(&pid, std::process::id().to_string()).unwrap();
    assert!(
        is_session_alive(&name),
        "live PID and listener must be reused"
    );

    // Reap a real child to obtain a dead PID without assuming a platform PID limit.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = child.id();
    child.wait().unwrap();
    std::fs::write(&pid, dead_pid.to_string()).unwrap();
    assert!(!is_session_alive(&name), "dead PID must not be reused");
    std::fs::write(&pid, "not-a-pid").unwrap();
    assert!(
        !is_session_alive(&name),
        "malformed PID file must not be reused"
    );
    std::fs::write(&pid, std::process::id().to_string()).unwrap();
    drop(listener);
    assert!(
        !is_session_alive(&name),
        "socket file without listener is stale"
    );
    std::fs::remove_file(&pid).unwrap();
    assert!(
        !is_session_alive(&name),
        "missing PID file must not be reused"
    );
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn browser_session_paths_follow_bridge_runtime_directory() {
    let _guard = crate::storage::lock_test_env();
    let expected = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    assert_eq!(
        session_socket_path("path-test"),
        expected.join("browser-session-path-test.sock")
    );
}
