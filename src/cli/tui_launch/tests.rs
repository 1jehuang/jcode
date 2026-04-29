#[cfg(unix)]
use super::{
    resumed_window_title, should_show_server_spawning, spawn_resume_in_new_terminal,
    spawn_selfdev_in_new_terminal,
};
#[cfg(unix)]
use crate::platform::set_permissions_executable;
#[cfg(unix)]
use crate::transport::Listener;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
}

#[cfg(unix)]
impl EnvVarGuard {
    fn set_path(key: &'static str, value: &Path) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }

    fn set_value(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

#[cfg(unix)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[cfg(unix)]
fn write_fake_handterm(temp: &tempfile::TempDir, output_path: &Path) {
    let script_path = temp.path().join("handterm");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$PWD\" > {}\nprintf '%s\\n' \"$@\" >> {}\n",
        output_path.display(),
        output_path.display()
    );
    fs::write(&script_path, script).expect("write fake handterm script");
    set_permissions_executable(&script_path).expect("make fake handterm executable");
}

#[cfg(unix)]
fn write_fake_tmux(temp: &tempfile::TempDir, output_path: &Path) {
    let script_path = temp.path().join("tmux");
    let script = format!(
        "#!/bin/sh\nprintf '%s\n' \"$PWD\" > {}\nprintf '%s\n' \"$@\" >> {}\n",
        output_path.display(),
        output_path.display()
    );
    fs::write(&script_path, script).expect("write fake tmux script");
    set_permissions_executable(&script_path).expect("make fake tmux executable");
}

#[cfg(unix)]
fn wait_for_lines(path: &Path, min_lines: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(content) = fs::read_to_string(path) {
            let lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
            if lines.len() >= min_lines {
                return lines;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for launcher output at {}",
        path.display()
    );
}

#[cfg(unix)]
#[test]
fn spawn_resume_in_new_terminal_uses_handterm_exec_mode() {
    let temp = tempfile::tempdir().expect("temp dir");
    let output_path = temp.path().join("resume-launch.txt");
    write_fake_handterm(&temp, &output_path);
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _path_guard = EnvVarGuard::set_value("PATH", &path);
    let _term_guard = EnvVarGuard::set_value("JCODE_TERMINAL", "handterm");

    let exe = temp.path().join("jcode-bin");
    let cwd = temp.path().join("cwd");
    fs::create_dir_all(&cwd).expect("create cwd");

    let launched =
        spawn_resume_in_new_terminal(&exe, "ses_test_123", &cwd).expect("spawn should work");
    assert!(launched);

    let lines = wait_for_lines(&output_path, 5);
    assert_eq!(
        Path::new(&lines[0]).canonicalize().expect("canonical pwd"),
        cwd.canonicalize().expect("canonical cwd")
    );
    assert_eq!(lines[1], "--standalone");
    assert_eq!(lines[2], "--backend");
    assert_eq!(lines[3], "gpu");
    assert_eq!(lines[4], "--exec");
    assert!(lines[5].contains("--resume"));
    assert!(lines[5].contains("ses_test_123"));
    assert!(lines[5].contains(exe.to_string_lossy().as_ref()));
}

#[cfg(unix)]
#[test]
fn spawn_resume_in_new_terminal_uses_tmux_new_window() {
    let temp = tempfile::tempdir().expect("temp dir");
    let output_path = temp.path().join("tmux-launch.txt");
    write_fake_tmux(&temp, &output_path);
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _path_guard = EnvVarGuard::set_value("PATH", &path);
    let _term_guard = EnvVarGuard::set_value("JCODE_TERMINAL", "tmux");

    let exe = temp.path().join("jcode bin");
    let cwd = temp.path().join("cwd");
    fs::create_dir_all(&cwd).expect("create cwd");

    let launched =
        spawn_resume_in_new_terminal(&exe, "ses_tmux_123", &cwd).expect("spawn should work");
    assert!(launched);

    let lines = wait_for_lines(&output_path, 7);
    assert_eq!(
        Path::new(&lines[0]).canonicalize().expect("canonical pwd"),
        cwd.canonicalize().expect("canonical cwd")
    );
    assert_eq!(lines[1], "new-window");
    assert_eq!(lines[2], "-c");
    assert_eq!(lines[3], cwd.to_string_lossy());
    assert_eq!(lines[4], "-n");
    assert_eq!(lines[5], resumed_window_title("ses_tmux_123"));
    assert!(lines[6].contains("'--fresh-spawn'"));
    assert!(lines[6].contains("'--resume'"));
    assert!(lines[6].contains("'ses_tmux_123'"));
    assert!(lines[6].contains("jcode bin"));
}

#[cfg(unix)]
#[test]
fn resumed_window_title_includes_server_name_when_registry_matches_socket() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::tempdir().expect("temp home");
    let temp_runtime = tempfile::tempdir().expect("temp runtime");
    let socket_path = temp_runtime.path().join("jcode.sock");
    let _home_guard = EnvVarGuard::set_path("JCODE_HOME", temp_home.path());
    let _socket_guard = EnvVarGuard::set_path("JCODE_SOCKET", &socket_path);

    let mut registry = crate::registry::ServerRegistry::default();
    registry.register(crate::registry::ServerInfo {
        id: "server_blazing_123".to_string(),
        name: "blazing".to_string(),
        icon: "🔥".to_string(),
        socket: socket_path,
        debug_socket: temp_runtime.path().join("jcode-debug.sock"),
        git_hash: "abc1234".to_string(),
        version: "v0.1.0".to_string(),
        pid: std::process::id(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        sessions: Vec::new(),
    });
    std::fs::create_dir_all(temp_home.path()).expect("create temp home");
    std::fs::write(
        crate::registry::registry_path().expect("registry path"),
        serde_json::to_string(&registry).expect("serialize registry"),
    )
    .expect("write registry");

    assert_eq!(
        resumed_window_title("session_parrot_123"),
        "🦜 jcode/blazing parrot"
    );
}

#[cfg(unix)]
#[test]
fn spawn_selfdev_in_new_terminal_uses_handterm_exec_mode() {
    let temp = tempfile::tempdir().expect("temp dir");
    let output_path = temp.path().join("selfdev-launch.txt");
    write_fake_handterm(&temp, &output_path);
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _path_guard = EnvVarGuard::set_value("PATH", &path);
    let _term_guard = EnvVarGuard::set_value("JCODE_TERMINAL", "handterm");

    let exe = temp.path().join("jcode-bin");
    let cwd = temp.path().join("cwd");
    fs::create_dir_all(&cwd).expect("create cwd");

    let launched =
        spawn_selfdev_in_new_terminal(&exe, "ses_selfdev_123", &cwd).expect("spawn should work");
    assert!(launched);

    let lines = wait_for_lines(&output_path, 5);
    assert_eq!(lines[0], cwd.to_string_lossy());
    assert_eq!(lines[1], "--standalone");
    assert_eq!(lines[2], "--backend");
    assert_eq!(lines[3], "gpu");
    assert_eq!(lines[4], "--exec");
    assert!(lines[5].contains("--resume"));
    assert!(lines[5].contains("ses_selfdev_123"));
    assert!(lines[5].contains("self-dev"));
    assert!(lines[5].contains(exe.to_string_lossy().as_ref()));
}

#[cfg(unix)]
#[tokio::test]
async fn suppresses_stale_server_spawning_phase_when_listener_is_already_live() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let socket_path = temp.path().join("jcode.sock");
    let _socket_guard = EnvVarGuard::set_path("JCODE_SOCKET", &socket_path);
    let _listener = Listener::bind(&socket_path).expect("bind listener");

    assert!(
        !should_show_server_spawning(true).await,
        "server startup banner should not linger once the listener is already live"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_server_spawning_phase_while_listener_is_not_live() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let socket_path = temp.path().join("jcode.sock");
    let _socket_guard = EnvVarGuard::set_path("JCODE_SOCKET", &socket_path);

    assert!(
        should_show_server_spawning(true).await,
        "server startup banner should still show before a listener exists"
    );
    assert!(
        !should_show_server_spawning(false).await,
        "server startup banner should stay hidden when client did not request it"
    );
}
