use super::*;
use std::os::unix::net::UnixListener;

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
