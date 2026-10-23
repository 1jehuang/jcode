//! Client for `jcode companion`, the per-login helper that owns desktop-only
//! actions: opening URLs and files, spawning terminal windows, the lid-close
//! override and global voice.
//!
//! The server and agent runtime never need a screen, keyboard, microphone or
//! lid. When they want one of those effects they ask the companion over a
//! local socket. A headless or remote server simply has no companion, and the
//! caller falls back to its built-in local behavior (or reports that nothing
//! could be shown).
//!
//! Wire format: one JSON [`CompanionRequest`] per line, answered by one JSON
//! [`CompanionResponse`] line.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;

/// Bumped when a request's meaning changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CompanionRequest {
    /// Liveness and capability probe.
    Status,
    /// Open a URL or local path with the user's default application.
    Open { target: String },
    /// Show a local path in the file manager.
    Reveal { path: PathBuf },
    /// Run a program in a new terminal window.
    SpawnTerminal {
        program: PathBuf,
        args: Vec<String>,
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// `JCODE_SPAWN_*`-style metadata exported to hooks and the terminal.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        /// Terminal env of the client that asked (tmux, zellij, ...), so the
        /// window lands in the terminal the user is attached to.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        client_terminal_env: Vec<(String, String)>,
    },
    /// Whether any session is actively working. The companion applies the
    /// configured lid-close override while this is true.
    PowerActivity { active: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionResponse {
    pub ok: bool,
    /// Human-readable result, e.g. which opener ran, or the error.
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub protocol: u32,
    /// Capabilities this companion handles, e.g. `open`, `voice`, `lid`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl CompanionResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            protocol: PROTOCOL_VERSION,
            capabilities: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            protocol: PROTOCOL_VERSION,
            capabilities: Vec::new(),
        }
    }
}

/// `$JCODE_COMPANION_SOCKET`, else `jcode-companion.sock` in the runtime dir.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("JCODE_COMPANION_SOCKET") {
        return PathBuf::from(path);
    }
    crate::storage::runtime_dir().join("jcode-companion.sock")
}

/// Why a request did not complete.
#[derive(Debug)]
pub enum CompanionError {
    /// No companion is listening. Callers should use their local fallback.
    Unavailable,
    /// The companion handled the request and reported a failure.
    Failed(String),
    /// The connection broke mid-request.
    Io(std::io::Error),
}

impl std::fmt::Display for CompanionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("jcode companion is not running"),
            Self::Failed(message) => f.write_str(message),
            Self::Io(error) => write!(f, "jcode companion connection failed: {error}"),
        }
    }
}

impl std::error::Error for CompanionError {}

/// Send one request and wait for the reply. Blocking, bounded by `timeout`.
pub fn request(
    request: &CompanionRequest,
    timeout: Duration,
) -> Result<CompanionResponse, CompanionError> {
    request_at(&socket_path(), request, timeout)
}

pub fn request_at(
    path: &std::path::Path,
    request: &CompanionRequest,
    timeout: Duration,
) -> Result<CompanionResponse, CompanionError> {
    let mut stream =
        jcode_transport::SyncStream::connect(path).map_err(|_| CompanionError::Unavailable)?;
    stream
        .set_read_timeout(Some(timeout.max(CONNECT_TIMEOUT)))
        .map_err(CompanionError::Io)?;
    let mut line = serde_json::to_string(request).map_err(|e| CompanionError::Io(e.into()))?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(CompanionError::Io)?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(CompanionError::Io)?;
    let response: CompanionResponse = serde_json::from_str(reply.trim())
        .map_err(|e| CompanionError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    if response.ok {
        Ok(response)
    } else {
        Err(CompanionError::Failed(response.message))
    }
}

/// Async wrapper that keeps the blocking socket I/O off the runtime.
pub async fn request_async(
    request: CompanionRequest,
    timeout: Duration,
) -> Result<CompanionResponse, CompanionError> {
    match tokio::task::spawn_blocking(move || self::request(&request, timeout)).await {
        Ok(result) => result,
        Err(error) => Err(CompanionError::Io(std::io::Error::other(error))),
    }
}

/// Open a URL for the user: via the companion when one is running, else with
/// this machine's opener. Returns whether something was launched.
///
/// Login flows run in the CLI process, which is usually on the user's desktop,
/// so the local fallback is the common path there.
pub fn open_url(url: &str) -> bool {
    // Never pop real windows (or reach a real companion) from test binaries.
    if crate::auth::running_in_test_harness() {
        return false;
    }
    match request(
        &CompanionRequest::Open {
            target: url.to_string(),
        },
        Duration::from_secs(10),
    ) {
        Ok(_) => true,
        Err(CompanionError::Unavailable) => open::that_detached(url).is_ok(),
        Err(_) => open::that_detached(url).is_ok(),
    }
}

/// Whether a companion answers right now.
pub fn is_running() -> bool {
    request(&CompanionRequest::Status, Duration::from_millis(500)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_tagged_json() {
        let request = CompanionRequest::SpawnTerminal {
            program: "/usr/bin/jcode".into(),
            args: vec!["--resume".into(), "ses_1".into()],
            cwd: "/tmp".into(),
            title: Some("jcode".into()),
            kind: Some("resume".into()),
            session_id: Some("ses_1".into()),
            env: vec![("JCODE_SPAWN_SWARM_ID".into(), "s".into())],
            client_terminal_env: vec![("TMUX".into(), "x".into())],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.starts_with("{\"op\":\"spawn_terminal\""));
        assert_eq!(
            serde_json::from_str::<CompanionRequest>(&json).unwrap(),
            request
        );
        let open: CompanionRequest =
            serde_json::from_str(r#"{"op":"open","target":"https://example.com"}"#).unwrap();
        assert_eq!(
            open,
            CompanionRequest::Open {
                target: "https://example.com".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_socket_reports_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let error = request_at(
            &dir.path().join("none.sock"),
            &CompanionRequest::Status,
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(matches!(error, CompanionError::Unavailable));
    }

    #[cfg(unix)]
    #[test]
    fn failed_reply_maps_to_failed_error() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            assert!(line.contains("\"op\":\"open\""));
            let reply = serde_json::to_string(&CompanionResponse::error("no opener")).unwrap();
            (&stream)
                .write_all(format!("{reply}\n").as_bytes())
                .unwrap();
        });
        let error = request_at(
            &path,
            &CompanionRequest::Open { target: "x".into() },
            Duration::from_secs(2),
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, CompanionError::Failed(ref m) if m == "no opener"));
    }
}
