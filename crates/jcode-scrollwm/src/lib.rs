//! A small, dependency-light client for the **ScrollWM** control plane.
//!
//! ScrollWM (a macOS scrolling window manager, <https://github.com/1jehuang/scrollwm>)
//! exposes a Unix-domain control socket. A client connects, writes one
//! newline-terminated command line, half-closes its write side, and reads one
//! reply line back. This crate is the Rust counterpart to ScrollWM's Swift
//! `ControlClient.send`, used by jcode to:
//!
//!   - detect whether ScrollWM is installed / running (`is_running`, `hello`),
//!   - read the live strip layout (`status`), and
//!   - drive it (`arrange`, `focus_index`, `focus_title`, `workspace`, ...),
//!     e.g. to tile headed swarm-agent windows into the strip.
//!
//! ## Design
//! - **Blocking + short-lived.** Each call is one connect/write/read round-trip
//!   on a local socket, mirroring the Swift CLI. Callers that need async wrap it
//!   in `spawn_blocking`.
//! - **Best-effort.** ScrollWM is optional: when it is absent the client returns
//!   [`ScrollWmError::NotRunning`] (mapped from `ENOENT`/`ECONNREFUSED`) and
//!   never panics, so jcode can degrade gracefully.
//! - **No side effects.** The client only ever connects out; it never launches
//!   ScrollWM (cold-start is a separate, opt-in concern) and never binds.
//! - **macOS-shaped, cross-platform-safe.** The Unix-socket path compiles on any
//!   `unix`; on non-unix targets the calls return `NotRunning` so callers don't
//!   need their own `cfg` gates.
//!
//! ## Wire contract (matches `ControlServer.swift` / `ControlCommands.swift`)
//! Request: `"<verb> [args]\n"`. Reply: one line; lines beginning with `error:`
//! denote a command-level failure. `status`/`version` reply with a JSON object.

use std::path::PathBuf;
use std::time::Duration;

/// Default per-call timeout for connect/read. Local socket round-trips are sub-
/// millisecond; this only guards against a wedged (main-thread-blocked) app so a
/// ScrollWM hiccup never stalls a jcode spawn.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(750);

/// Environment override for the control socket path (matches ScrollWM's
/// `SCROLLWM_CONTROL_SOCK`, used by its sandbox mode and by jcode tests).
pub const SOCKET_ENV: &str = "SCROLLWM_CONTROL_SOCK";

/// Errors talking to the ScrollWM control socket.
#[derive(Debug)]
pub enum ScrollWmError {
    /// No socket file (`ENOENT`) or a stale socket (`ECONNREFUSED`): ScrollWM is
    /// not running. This is the expected "absent" case, not a hard error.
    NotRunning,
    /// A lower-level I/O failure (connect/write/read/timeout) other than the
    /// not-running case above.
    Io(std::io::Error),
    /// The server replied with an `error:`-prefixed line (command-level failure).
    Protocol(String),
    /// A reply that should have been JSON (`status`, `version`) could not be
    /// parsed.
    Parse(String),
    /// This platform has no Unix-domain ScrollWM socket (non-unix targets).
    Unsupported,
}

impl std::fmt::Display for ScrollWmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScrollWmError::NotRunning => write!(f, "ScrollWM is not running"),
            ScrollWmError::Io(e) => write!(f, "ScrollWM I/O error: {e}"),
            ScrollWmError::Protocol(m) => write!(f, "ScrollWM error: {m}"),
            ScrollWmError::Parse(m) => write!(f, "ScrollWM reply parse error: {m}"),
            ScrollWmError::Unsupported => write!(f, "ScrollWM is not supported on this platform"),
        }
    }
}

impl std::error::Error for ScrollWmError {}

/// Resolve the control socket path the same way ScrollWM does:
/// `$SCROLLWM_CONTROL_SOCK`, else
/// `~/Library/Application Support/ScrollWM/control.sock`.
pub fn control_socket_path() -> PathBuf {
    if let Some(override_path) = std::env::var_os(SOCKET_ENV)
        && !override_path.is_empty()
    {
        return PathBuf::from(override_path);
    }
    // ScrollWM uses the macOS Application Support directory. `dirs` resolves the
    // platform's data dir; on macOS that is exactly Library/Application Support.
    let base = dirs::data_dir().unwrap_or_else(|| {
        // Fallback for the unusual case `dirs` can't resolve a data dir.
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join("Library/Application Support")
    });
    base.join("ScrollWM").join("control.sock")
}

/// A handle to the ScrollWM control socket. Cheap to construct and clone; each
/// method opens a fresh short-lived connection.
#[derive(Clone, Debug)]
pub struct ScrollWm {
    socket: PathBuf,
    timeout: Duration,
}

impl Default for ScrollWm {
    fn default() -> Self {
        Self::discover()
    }
}

impl ScrollWm {
    /// Construct using the default (env-aware) socket path and timeout.
    pub fn discover() -> Self {
        Self {
            socket: control_socket_path(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Construct against an explicit socket path (sandbox / tests).
    pub fn with_socket(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the per-call timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The resolved socket path this client talks to.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket
    }

    /// Send one verb line and return the trimmed reply. An `error:`-prefixed
    /// reply is surfaced as [`ScrollWmError::Protocol`]. Connecting to a missing
    /// or stale socket yields [`ScrollWmError::NotRunning`].
    pub fn send(&self, line: &str) -> Result<String, ScrollWmError> {
        let reply = send_raw(&self.socket, line, self.timeout)?;
        if let Some(rest) = reply.strip_prefix("error:") {
            return Err(ScrollWmError::Protocol(rest.trim().to_string()));
        }
        Ok(reply)
    }

    /// Liveness probe: `ping` -> `pong`. False on any error (not running, I/O,
    /// timeout), so callers can branch without matching the error.
    pub fn is_running(&self) -> bool {
        matches!(self.send("ping"), Ok(reply) if reply == "pong")
    }

    /// Capability handshake. Prefers the `version` verb (newer ScrollWM); when
    /// that verb is unknown (older builds reply `error: unknown command`), falls
    /// back to deriving `{managing, ...}` presence from `status` with
    /// `protocol = 0` so feature detection still degrades gracefully.
    pub fn hello(&self) -> Result<ScrollWmInfo, ScrollWmError> {
        match self.send("version") {
            Ok(reply) => serde_json::from_str::<ScrollWmInfo>(&reply)
                .map_err(|e| ScrollWmError::Parse(e.to_string())),
            // Older ScrollWM without the `version` verb: synthesize a v0 info
            // from a successful `status` so callers still get a usable handshake.
            Err(ScrollWmError::Protocol(_)) => {
                self.status()?;
                Ok(ScrollWmInfo {
                    name: "ScrollWM".to_string(),
                    version: String::new(),
                    protocol: 0,
                    verbs: Vec::new(),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Read the live strip status (managing flag, columns, workspace, ...).
    pub fn status(&self) -> Result<StripStatus, ScrollWmError> {
        let reply = self.send("status")?;
        serde_json::from_str::<StripStatus>(&reply).map_err(|e| ScrollWmError::Parse(e.to_string()))
    }

    /// Adopt the current Space's windows into the strip (idempotent: also
    /// re-syncs when already managing). NOTE: this affects *every* manageable
    /// window on the Space, so callers should only invoke it intentionally.
    pub fn arrange(&self) -> Result<String, ScrollWmError> {
        self.send("arrange")
    }

    /// Focus a column by 1-based index (the CLI is 1-based; the engine is
    /// 0-based internally).
    pub fn focus_index(&self, one_based: usize) -> Result<String, ScrollWmError> {
        self.send(&format!("focus {one_based}"))
    }

    /// Focus the next / previous column.
    pub fn focus_next(&self) -> Result<String, ScrollWmError> {
        self.send("focus next")
    }

    /// Focus the previous column.
    pub fn focus_prev(&self) -> Result<String, ScrollWmError> {
        self.send("focus prev")
    }

    /// Focus the managed column whose window title contains `needle`
    /// (case-insensitive), resolved from a fresh [`status`](Self::status) read.
    ///
    /// jcode names its spawned terminal windows deterministically, so matching by
    /// title is the reliable way to focus a specific agent without depending on
    /// volatile column indices. Returns [`ScrollWmError::Protocol`] when nothing
    /// matches (mirroring how the native verbs report "not found").
    pub fn focus_title(&self, needle: &str) -> Result<String, ScrollWmError> {
        let status = self.status()?;
        match column_for_title(&status, needle) {
            Some(index) => self.focus_index(index),
            None => Err(ScrollWmError::Protocol(format!(
                "no managed column title contains \"{needle}\""
            ))),
        }
    }
}

/// One short-lived connect/write/read round-trip against a Unix socket path.
/// Writes `line` (adding a trailing newline if absent), half-closes the write
/// side, reads the reply to EOF, and returns it trimmed. ENOENT/ECONNREFUSED map
/// to [`ScrollWmError::NotRunning`].
#[cfg(unix)]
pub fn send_raw(
    socket: &std::path::Path,
    line: &str,
    timeout: Duration,
) -> Result<String, ScrollWmError> {
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;

    let mut stream = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(e) => {
            return Err(match e.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                    ScrollWmError::NotRunning
                }
                _ => ScrollWmError::Io(e),
            });
        }
    };
    stream
        .set_read_timeout(Some(timeout))
        .map_err(ScrollWmError::Io)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(ScrollWmError::Io)?;

    let mut msg = line.to_string();
    if !msg.ends_with('\n') {
        msg.push('\n');
    }
    stream
        .write_all(msg.as_bytes())
        .map_err(ScrollWmError::Io)?;
    // Half-close so the server sees EOF and flushes its single-line reply.
    let _ = stream.shutdown(Shutdown::Write);

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(ScrollWmError::Io)?;
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

/// Non-unix stub: there is no ScrollWM socket, so every call is "not running".
#[cfg(not(unix))]
pub fn send_raw(
    _socket: &std::path::Path,
    _line: &str,
    _timeout: Duration,
) -> Result<String, ScrollWmError> {
    Err(ScrollWmError::Unsupported)
}

/// The capability handshake (`version` verb), used for feature detection. New
/// fields are additive; unknown ones are ignored.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
pub struct ScrollWmInfo {
    /// Product name (e.g. "ScrollWM").
    #[serde(default)]
    pub name: String,
    /// Marketing version (e.g. "0.1.6"); display-only.
    #[serde(default)]
    pub version: String,
    /// Monotonic control-protocol revision; the coarse compatibility gate.
    #[serde(default)]
    pub protocol: u32,
    /// Supported verb names, for fine-grained feature detection.
    #[serde(default)]
    pub verbs: Vec<String>,
}

impl ScrollWmInfo {
    /// Whether this ScrollWM advertises a given verb (only meaningful when the
    /// `verbs` list is populated, i.e. `version` was supported).
    pub fn supports_verb(&self, verb: &str) -> bool {
        self.verbs.iter().any(|v| v == verb)
    }
}

/// Snapshot of the strip returned by the `status` verb. Mirrors
/// `controlStatusJSON()` / `controlColumns()` in ScrollWM. Optional fields are
/// only present while managing.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
pub struct StripStatus {
    /// Whether ScrollWM is actively managing windows (vs. dormant).
    #[serde(default)]
    pub managing: bool,
    /// Current focus mode ("fit" / "centered"), display-only.
    #[serde(default, rename = "focusMode")]
    pub focus_mode: String,
    /// Total managed window count.
    #[serde(default, rename = "windowCount")]
    pub window_count: u32,
    /// 1-based index of the focused column, when managing.
    #[serde(default, rename = "focusedColumn")]
    pub focused_column: Option<u32>,
    /// Current vertical workspace (1-based), when managing.
    #[serde(default)]
    pub workspace: Option<u32>,
    /// Total vertical workspace count, when managing.
    #[serde(default, rename = "workspaceCount")]
    pub workspace_count: Option<u32>,
    /// The columns of the active strip, left to right.
    #[serde(default)]
    pub columns: Vec<Column>,
}

/// One column (window) of the strip.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
pub struct Column {
    /// 1-based column index.
    #[serde(default)]
    pub index: u32,
    /// Owning application name (e.g. "Ghostty").
    #[serde(default)]
    pub app: String,
    /// Window title (jcode sets a unique per-session title here).
    #[serde(default)]
    pub title: String,
    /// Column width in pixels.
    #[serde(default)]
    pub width: u32,
    /// Whether this column currently has focus.
    #[serde(default)]
    pub focused: bool,
    /// Whether ScrollWM considers this window healthy/managed.
    #[serde(default)]
    pub healthy: bool,
}

/// Pure helper: find the 1-based index of the first column whose title contains
/// `needle` (case-insensitive). Returns `None` when nothing matches. Extracted
/// for unit testing without a live ScrollWM.
pub fn column_for_title(status: &StripStatus, needle: &str) -> Option<usize> {
    let needle = needle.to_ascii_lowercase();
    status
        .columns
        .iter()
        .find(|c| c.title.to_ascii_lowercase().contains(&needle))
        .map(|c| c.index as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_honors_env_override() {
        // Safe: serial within this test; restores the var afterward.
        let prev = std::env::var_os(SOCKET_ENV);
        unsafe { std::env::set_var(SOCKET_ENV, "/tmp/scrollwm-test.sock") };
        assert_eq!(
            control_socket_path(),
            PathBuf::from("/tmp/scrollwm-test.sock")
        );
        match prev {
            Some(v) => unsafe { std::env::set_var(SOCKET_ENV, v) },
            None => unsafe { std::env::remove_var(SOCKET_ENV) },
        }
    }

    #[test]
    fn default_socket_path_ends_with_scrollwm_control_sock() {
        let prev = std::env::var_os(SOCKET_ENV);
        unsafe { std::env::remove_var(SOCKET_ENV) };
        let path = control_socket_path();
        assert!(
            path.ends_with("ScrollWM/control.sock"),
            "unexpected default socket path: {}",
            path.display()
        );
        if let Some(v) = prev {
            unsafe { std::env::set_var(SOCKET_ENV, v) };
        }
    }

    fn status_with_titles(titles: &[&str]) -> StripStatus {
        StripStatus {
            managing: true,
            columns: titles
                .iter()
                .enumerate()
                .map(|(i, t)| Column {
                    index: (i + 1) as u32,
                    app: "Ghostty".to_string(),
                    title: (*t).to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn column_for_title_matches_case_insensitively() {
        let status = status_with_titles(&["🛰 jcode/aqua main", "Browser", "🛰 jcode/coral docs"]);
        assert_eq!(column_for_title(&status, "jcode/AQUA"), Some(1));
        assert_eq!(column_for_title(&status, "coral"), Some(3));
        assert_eq!(column_for_title(&status, "nope"), None);
    }

    #[test]
    fn status_json_round_trips_control_shape() {
        // The exact field names ScrollWM emits (camelCase) must deserialize.
        let json = r#"{
            "managing": true,
            "focusMode": "fit",
            "windowCount": 2,
            "focusedColumn": 1,
            "workspace": 1,
            "workspaceCount": 1,
            "columns": [
                {"index": 1, "app": "Ghostty", "title": "🛰 jcode/aqua", "width": 800, "focused": true, "healthy": true},
                {"index": 2, "app": "Safari", "title": "Docs", "width": 600, "focused": false, "healthy": true}
            ]
        }"#;
        let status: StripStatus = serde_json::from_str(json).expect("parse status");
        assert!(status.managing);
        assert_eq!(status.window_count, 2);
        assert_eq!(status.focused_column, Some(1));
        assert_eq!(status.columns.len(), 2);
        assert_eq!(column_for_title(&status, "aqua"), Some(1));
    }

    #[test]
    fn version_json_parses_capabilities() {
        let json = r#"{"name":"ScrollWM","version":"0.2.0","protocol":1,"verbs":["ping","status","arrange","focus-title"]}"#;
        let info: ScrollWmInfo = serde_json::from_str(json).expect("parse version");
        assert_eq!(info.protocol, 1);
        assert!(info.supports_verb("focus-title"));
        assert!(!info.supports_verb("spawn-strip"));
    }

    #[test]
    fn missing_socket_reports_not_running() {
        let client = ScrollWm::with_socket("/tmp/jcode-scrollwm-nonexistent-12345.sock");
        assert!(!client.is_running());
        match client.status() {
            Err(ScrollWmError::NotRunning) => {}
            other => panic!("expected NotRunning, got {other:?}"),
        }
    }
}

#[cfg(all(test, unix))]
mod loopback_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    /// Spin up a one-shot Unix listener that replies with `reply` to the first
    /// connection, so we can exercise the real connect/write/read path without a
    /// live ScrollWM. Returns the socket path; the listener thread self-cleans.
    fn spawn_echo_server(reply: &'static str) -> PathBuf {
        // Keep the path SHORT: macOS sockaddr_un.sun_path is capped at 104
        // bytes, and the default temp dir (/var/folders/.../T/) is already long.
        // Use /tmp + a process-unique atomic counter so parallel tests never
        // collide and never overflow the limit.
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!(
            "/tmp/jcode-sw-lb-{}-{}.sock",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind loopback socket");
        let thread_path = path.clone();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request (until the client half-closes its write side).
                let mut req = Vec::new();
                let mut buf = [0u8; 256];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                }
                let _ = stream.write_all(reply.as_bytes());
            }
            let _ = std::fs::remove_file(&thread_path);
        });
        path
    }

    #[test]
    fn ping_round_trips_over_loopback() {
        let path = spawn_echo_server("pong\n");
        let client = ScrollWm::with_socket(path);
        assert!(client.is_running());
    }

    #[test]
    fn error_reply_maps_to_protocol_error() {
        let path = spawn_echo_server("error: not managing\n");
        let client = ScrollWm::with_socket(path);
        match client.send("focus 1") {
            Err(ScrollWmError::Protocol(m)) => assert_eq!(m, "not managing"),
            other => panic!("expected Protocol error, got {other:?}"),
        }
    }

    #[test]
    fn status_reply_parses_over_loopback() {
        let path = spawn_echo_server(
            r#"{"managing":true,"windowCount":1,"columns":[{"index":1,"app":"Ghostty","title":"jcode/x","focused":true}]}"#,
        );
        let client = ScrollWm::with_socket(path);
        let status = client.status().expect("status");
        assert!(status.managing);
        assert_eq!(status.columns.len(), 1);
    }
}
