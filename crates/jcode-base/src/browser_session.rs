//! Per-agent browser session daemons (`browser session start`).
//!
//! A daemon holds one WebSocket to the native host for the lifetime of a jcode
//! session, so it is bound to the host of one browser. Several browsers can run
//! the bridge at once (each host takes its own port, see #1720), so the daemon
//! name includes the target browser and the daemon is started with
//! `FAB_BROWSER` set, which the bridge CLI uses to pick that browser's host.

use super::*;

fn runtime_dir() -> PathBuf {
    storage::runtime_dir()
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
        && platform::is_process_running(pid)
    {
        return session_socket_path(name).exists();
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

    let bin = browser_binary_path();
    if !bin.exists() {
        return None;
    }

    // Bind each agent session to a dedicated browser window when the installed
    // bridge supports it. Older bridge CLIs reject --bind-window, so probe the
    // command surface instead of paying for a known-failing process launch on
    // every browser action.
    if browser_supports_bind_window(&bin)
        && let Some(name) = spawn_browser_session(&bin, &session_name, browser, true)
    {
        return Some(name);
    }
    spawn_browser_session(&bin, &session_name, browser, false)
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
                        "[browser] session '{}' exited before startup with status {}{}",
                        session_name,
                        status,
                        if bind_window {
                            " (retrying without --bind-window)"
                        } else {
                            ""
                        }
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
