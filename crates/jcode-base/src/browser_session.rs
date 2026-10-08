//! Per-agent browser session daemons (`browser session start`).
//!
//! A daemon holds one WebSocket to the native host for the lifetime of a jcode
//! session, so it is bound to the host of one browser. Several browsers can run
//! the bridge at once (each host takes its own port, see #1720), so the daemon
//! name includes the target browser and the daemon is started with
//! `FAB_BROWSER` set, which the bridge CLI uses to pick that browser's host.

use super::*;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

fn runtime_dir() -> PathBuf {
    // Match the bridge CLI, which does not use JCODE_RUNTIME_DIR or TMPDIR.
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn session_socket_path(name: &str) -> PathBuf {
    runtime_dir().join(format!("browser-session-{}.sock", name))
}

fn session_pid_path(name: &str) -> PathBuf {
    runtime_dir().join(format!("browser-session-{}.pid", name))
}

fn is_session_alive(name: &str) -> bool {
    let pid_path = session_pid_path(name);
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
        && pid > 0
        && pid <= i32::MAX as u32
        && platform::is_process_running(pid)
    {
        #[cfg(unix)]
        return std::os::unix::net::UnixStream::connect(session_socket_path(name)).is_ok();
    }
    false
}

pub fn ensure_browser_session(session_id: &str) -> Option<String> {
    ensure_browser_session_for(session_id, None)
}

/// Session daemon for `session_id` talking to the host of `browser` (a bridge
/// browser name such as `chrome`, or `None` for the bridge's default host).
pub fn ensure_browser_session_for(session_id: &str, browser: Option<&str>) -> Option<String> {
    let session_name = session_name_for(session_id, browser);

    if is_session_alive(&session_name) {
        return Some(session_name);
    }

    // Only calls for the same daemon should wait for its startup.
    static STARTUP_LOCKS: LazyLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let startup_lock = {
        let mut locks = STARTUP_LOCKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&session_name).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(Mutex::new(()));
            locks.insert(session_name.clone(), Arc::downgrade(&lock));
            lock
        }
    };
    let _guard = startup_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if is_session_alive(&session_name) {
        return Some(session_name);
    }

    let bin = browser_binary_path();
    if !bin.exists() {
        return None;
    }

    // Bind each agent session to a dedicated browser window when the installed
    // bridge supports it. Older bridge CLIs reject --bind-window, so probe the
    // command surface instead of paying for a known-failing process launch on
    // every browser action.
    spawn_browser_session(
        &bin,
        &session_name,
        browser,
        browser_supports_bind_window(&bin),
    )
}

/// Keep process startup, socket checks, and lock waits off async workers.
pub async fn ensure_browser_session_for_async(
    session_id: &str,
    browser: Option<&str>,
) -> Option<String> {
    let session_id = session_id.to_owned();
    let browser = browser.map(str::to_owned);
    match tokio::task::spawn_blocking(move || {
        ensure_browser_session_for(&session_id, browser.as_deref())
    })
    .await
    {
        Ok(session) => session,
        Err(error) => {
            eprintln!("[browser] Session startup worker failed: {}", error);
            None
        }
    }
}

fn browser_supports_bind_window(bin: &std::path::Path) -> bool {
    std::process::Command::new(bin)
        .args(["session", "start", "--help"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout).contains("--bind-window")
                || String::from_utf8_lossy(&output.stderr).contains("--bind-window")
        })
}

fn spawn_browser_session(
    bin: &std::path::Path,
    session_name: &str,
    browser: Option<&str>,
    bind_window: bool,
) -> Option<String> {
    let mut args = vec!["session", "start", session_name];
    if bind_window {
        args.push("--bind-window");
    }
    let mut command = std::process::Command::new(bin);
    apply_bridge_browser_env(&mut command, browser);
    let result = command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(mut child) => {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if session_socket_path(session_name).exists() && is_session_alive(session_name) {
                    let _ = child.stdout.take();
                    return Some(session_name.to_string());
                }
                if let Ok(Some(status)) = child.try_wait() {
                    eprintln!(
                        "[browser] session '{}' exited before startup with status {}",
                        session_name, status
                    );
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            eprintln!(
                "[browser] session '{}' did not start within 10s",
                session_name
            );
            let _ = child.kill();
            let _ = child.wait();
            None
        }
        Err(e) => {
            eprintln!(
                "[browser] Failed to start browser session '{}': {}",
                session_name, e
            );
            None
        }
    }
}

/// Daemon name: the jcode session, plus the browser when one is targeted, so
/// switching browsers starts a daemon bound to the other browser's host.
pub(super) fn session_name_for(session_id: &str, browser: Option<&str>) -> String {
    let base = sanitize_session_name(session_id);
    match browser.map(sanitize_session_name).filter(|b| !b.is_empty()) {
        Some(browser) => format!("{}-{}", base, browser),
        None => base,
    }
}

fn sanitize_session_name(session_id: &str) -> String {
    session_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect()
}

#[cfg(all(test, unix))]
#[path = "browser_session_tests.rs"]
mod tests;
