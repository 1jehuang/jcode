//! Starting the harness when it is not already running.
//!
//! Ported from desktop2's `ensure_runtime`, which every embedding app would
//! otherwise have to write again. Counterpart to the TypeScript SDK's
//! `launch.ts`: the Rust version attaches to the user's own jcode rather than
//! provisioning a private instance, because its callers are desktop clients
//! that want the user's live sessions.

use crate::errors::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How the runtime should be started, when it needs to be.
pub struct LaunchOptions {
    /// How long to wait for a freshly spawned runtime to publish its socket.
    pub start_timeout: Duration,
    /// Path to the API socket. Defaults to the shared, resolved location.
    pub api_socket: Option<PathBuf>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            start_timeout: Duration::from_secs(30),
            api_socket: None,
        }
    }
}

/// Progress while a runtime comes up, so a UI can say what it is waiting for.
pub type Progress<'a> = dyn Fn(&str) + 'a;

/// Whether anything is listening on a socket path.
pub fn socket_accepts(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Locate a sibling executable next to our own, falling back to `$PATH`.
///
/// Self-dev and release builds both keep the binaries side by side, so a
/// sibling lookup starts the *matching* build instead of whatever stale copy
/// happens to be first on `$PATH`.
fn sibling_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Start the daemon and the API bridge if they are not already listening.
///
/// Idempotent and safe to race: both the daemon and the bridge refuse to
/// replace a live socket, so a duplicate spawn simply exits.
pub fn ensure_runtime(options: &LaunchOptions, progress: &Progress<'_>) -> Result<()> {
    let api = options
        .api_socket
        .clone()
        .unwrap_or_else(jcode_harness_api::api_socket_path);
    if socket_accepts(&api) {
        return Ok(());
    }

    let legacy = jcode_harness_api::legacy_socket_path();
    if !socket_accepts(&legacy) {
        progress("starting jcode runtime...");
        spawn_detached(&sibling_exe("jcode"), &["serve"]).map_err(|error| {
            launch_failed(format!("could not start the jcode runtime: {error}"))
        })?;
        wait_for_socket(&legacy, "jcode runtime", options.start_timeout)?;
    }

    progress("starting harness API bridge...");
    spawn_detached(&sibling_exe("jcode-harness-api-bridge"), &[]).map_err(|error| {
        launch_failed(format!("could not start jcode-harness-api-bridge: {error}"))
    })?;
    wait_for_socket(&api, "harness API bridge", options.start_timeout)
}

fn spawn_detached(program: &Path, args: &[&str]) -> std::io::Result<()> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

/// Block until `path` accepts connections, or the timeout expires.
pub fn wait_for_socket(path: &Path, what: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if socket_accepts(path) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(launch_failed(format!(
        "timed out waiting for the {what} at {}",
        path.display()
    )))
}

fn launch_failed(message: String) -> Error {
    Error::new(ErrorKind::LaunchFailed, message)
}
