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
    let browser = match browser {
        Some(name) => match BrowserKind::parse(name) {
            Some(kind) if kind.family() != BrowserFamily::Safari && kind.supported_on_this_os() => {
                Some(kind.id())
            }
            _ => {
                eprintln!(
                    "[browser] Unsupported bridge session target '{name}'; use a supported browser provider"
                );
                return None;
            }
        },
        None => None,
    };
    let session_name = session_name_for(session_id, browser);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if session_socket_path(&session_name)
            .as_os_str()
            .as_bytes()
            .len()
            >= 104
        {
            eprintln!(
                "[browser] Browser session socket path is too long for Unix sockets. Set XDG_RUNTIME_DIR to a shorter existing directory; refusing an unbound session."
            );
            return None;
        }
    }

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

fn prevent_inherited_cargo_gate(
    command: &mut std::process::Command,
    gate: Option<&std::path::Path>,
) {
    #[cfg(unix)]
    if let Some(metadata) = gate.and_then(|path| std::fs::metadata(path).ok()) {
        use std::os::unix::{fs::MetadataExt, process::CommandExt};
        let (device, inode) = (metadata.dev(), metadata.ino());
        // Only child state changes. fstat/fcntl are async-signal-safe, and an
        // unrelated descriptor 9 is preserved even if an environment flag lied.
        unsafe {
            command.pre_exec(move || {
                let mut stat: libc::stat = std::mem::zeroed();
                if libc::fstat(9, &mut stat) == 0
                    && stat.st_dev as u64 == device
                    && stat.st_ino as u64 == inode
                {
                    let flags = libc::fcntl(9, libc::F_GETFD);
                    if flags >= 0 && libc::fcntl(9, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = (command, gate);
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
    let gate = if std::env::var("JCODE_CARGO_GATE_HELD").as_deref() == Ok("1") {
        std::env::var_os("JCODE_CARGO_GATE_PATH")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let dir = std::env::var_os("JCODE_CARGO_GATE_DIR")
                    .or_else(|| std::env::var_os("XDG_RUNTIME_DIR"))
                    .or_else(|| std::env::var_os("TMPDIR"))
                    .unwrap_or_else(|| "/tmp".into());
                Some(std::path::PathBuf::from(dir).join("jcode-cargo-build.lock"))
            })
    } else {
        None
    };
    prevent_inherited_cargo_gate(&mut command, gate.as_deref());
    command.env_remove("BROWSER_SESSION");
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
    let name = match browser.map(sanitize_session_name).filter(|b| !b.is_empty()) {
        // Old targeted daemons could have inherited FAB_BROWSER for a different
        // browser or searched the agent's scratch TMPDIR registry. Do not reuse
        // those after enforcing explicit identity and the system temp directory.
        Some(browser) => format!("{}-{}-target-v4", base, browser),
        None => base,
    };
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let capacity =
            103_usize.saturating_sub(session_socket_path("").as_os_str().as_bytes().len());
        if name.len() > capacity {
            use sha2::{Digest, Sha256};
            let digest = hex::encode(Sha256::digest(name.as_bytes()));
            // Hash the whole browser/version-specific key when a longer
            // XDG_RUNTIME_DIR leaves less space for a readable session ID.
            let prefix = capacity.saturating_sub(17).min(name.len());
            return format!("{}-{}", &name[..prefix], &digest[..16]);
        }
    }
    name
}

fn sanitize_session_name(session_id: &str) -> String {
    let sanitized: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        // Keep the complete browser-specific name short enough for macOS's
        // 104-byte sockaddr_un.sun_path, including /tmp and socket affixes.
        .take(32)
        .collect();
    if sanitized == session_id {
        sanitized
    } else {
        use sha2::{Digest, Sha256};
        let digest = hex::encode(Sha256::digest(session_id.as_bytes()));
        format!("{}-{}", sanitized, &digest[..12])
    }
}

#[cfg(all(test, unix))]
#[path = "browser_session_tests.rs"]
mod tests;
