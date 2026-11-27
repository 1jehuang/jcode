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

use jcode_terminal_launch::RUNTIME_ENV_VARS;
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
    /// Runtime-context spawns use a distinct operation so an older companion
    /// rejects the request instead of accepting and silently dropping identity.
    SpawnTerminalWithRuntime {
        program: PathBuf,
        args: Vec<String>,
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        client_terminal_env: Vec<(String, String)>,
        #[serde(default)]
        fresh_spawn: bool,
        #[serde(default, with = "runtime_env_wire")]
        runtime_env: Vec<(String, String)>,
    },
    /// Whether any session is actively working. The companion applies the
    /// configured lid-close override while this is true.
    PowerActivity { active: bool },
}

impl CompanionRequest {
    /// Build the wire-compatible terminal request, retaining the original
    /// operation for callers without explicit runtime context.
    pub fn spawn_terminal(
        command: &crate::terminal_launch::TerminalCommand,
        cwd: &std::path::Path,
    ) -> Self {
        let runtime_env = command
            .runtime_env
            .iter()
            .filter(|(key, _)| RUNTIME_ENV_VARS.contains(&key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if runtime_env.is_empty() {
            Self::SpawnTerminal {
                program: command.program.clone(),
                args: command.args.clone(),
                cwd: cwd.to_path_buf(),
                title: command.title.clone(),
                kind: command.kind.clone(),
                session_id: command.session_id.clone(),
                env: command.extra_env.clone(),
                client_terminal_env: command.client_terminal_env.clone(),
            }
        } else {
            Self::SpawnTerminalWithRuntime {
                program: command.program.clone(),
                args: command.args.clone(),
                cwd: cwd.to_path_buf(),
                title: command.title.clone(),
                kind: command.kind.clone(),
                session_id: command.session_id.clone(),
                env: command.extra_env.clone(),
                client_terminal_env: command.client_terminal_env.clone(),
                fresh_spawn: command.fresh_spawn,
                runtime_env,
            }
        }
    }
}
mod runtime_env_wire {
    use jcode_terminal_launch::RUNTIME_ENV_VARS;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(env: &[(String, String)], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let filtered = env
            .iter()
            .filter(|(key, _)| RUNTIME_ENV_VARS.contains(&key.as_str()))
            .collect::<Vec<_>>();
        filtered.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<(String, String)>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Vec::<(String, String)>::deserialize(deserializer)?
            .into_iter()
            .filter(|(key, _)| RUNTIME_ENV_VARS.contains(&key.as_str()))
            .collect())
    }
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

/// Error suffix for a request whose feature is off in `[companion]` config.
/// Clients treat it like an absent companion.
pub const DISABLED_SUFFIX: &str = "is disabled in [companion] config";

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
    } else if response.message.ends_with(DISABLED_SUFFIX) {
        // The user turned this feature off: behave as if no companion runs,
        // so callers use their local fallback.
        Err(CompanionError::Unavailable)
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

    #[test]
    fn contextual_terminal_requests_use_a_distinct_filtered_wire_variant() {
        let command = crate::terminal_launch::TerminalCommand::new(
            "/usr/bin/jcode",
            vec!["--resume".into(), "ses_context".into()],
        )
        .title("context window")
        .kind("swarm-agent")
        .session_id("ses_context")
        .fresh_spawn()
        .spawn_env("JCODE_SPAWN_SWARM_ID", "swarm-1")
        .client_terminal_env(vec![("TMUX_PANE".into(), "%12".into())])
        .runtime_env(vec![
            ("JCODE_HOME".into(), "/tmp/jcode home".into()),
            ("JCODE_SOCKET".into(), "/tmp/connected.sock".into()),
            ("JCODE_API_KEY".into(), "never-send".into()),
        ]);
        let request = CompanionRequest::spawn_terminal(&command, std::path::Path::new("/work"));
        let CompanionRequest::SpawnTerminalWithRuntime {
            program,
            args,
            cwd,
            title,
            kind,
            session_id,
            env,
            client_terminal_env,
            fresh_spawn,
            runtime_env,
        } = request
        else {
            panic!("runtime identity must select the explicit contextual variant");
        };
        assert_eq!(program, PathBuf::from("/usr/bin/jcode"));
        assert_eq!(args, ["--resume", "ses_context"]);
        assert_eq!(cwd, PathBuf::from("/work"));
        assert_eq!(title.as_deref(), Some("context window"));
        assert_eq!(kind.as_deref(), Some("swarm-agent"));
        assert_eq!(session_id.as_deref(), Some("ses_context"));
        assert!(fresh_spawn);
        assert_eq!(env, [("JCODE_SPAWN_SWARM_ID".into(), "swarm-1".into())]);
        assert_eq!(client_terminal_env, [("TMUX_PANE".into(), "%12".into())]);
        assert_eq!(
            runtime_env,
            [
                ("JCODE_HOME".into(), "/tmp/jcode home".into()),
                ("JCODE_SOCKET".into(), "/tmp/connected.sock".into()),
            ]
        );

        let contextual = CompanionRequest::SpawnTerminalWithRuntime {
            program,
            args,
            cwd,
            title,
            kind,
            session_id,
            env,
            client_terminal_env,
            fresh_spawn,
            runtime_env: vec![
                ("JCODE_SOCKET".into(), "/tmp/connected.sock".into()),
                ("JCODE_API_KEY".into(), "must-filter-at-wire".into()),
            ],
        };
        let wire = serde_json::to_value(&contextual).unwrap();
        assert_eq!(wire["op"], "spawn_terminal_with_runtime");
        assert_eq!(
            wire["runtime_env"],
            serde_json::json!([["JCODE_SOCKET", "/tmp/connected.sock"]])
        );
        let old_spawn = serde_json::to_value(CompanionRequest::spawn_terminal(
            &crate::terminal_launch::TerminalCommand::new("jcode", vec!["--resume".into()]),
            std::path::Path::new("/work"),
        ))
        .unwrap();
        assert_eq!(old_spawn["op"], "spawn_terminal");
        assert!(old_spawn.get("runtime_env").is_none());
        assert!(
            serde_json::from_str::<CompanionRequest>(
                r#"{"op":"spawn_terminal","program":"jcode","args":[],"cwd":"/work"}"#
            )
            .is_ok()
        );

        let unsupported_variant = serde_json::to_value(&contextual).unwrap();
        assert_eq!(unsupported_variant["op"], "spawn_terminal_with_runtime");
        let old_peer_error = serde_json::from_value::<OldCompanionRequest>(unsupported_variant)
            .expect_err("old peers reject unknown operation names");
        assert!(old_peer_error.to_string().contains("unknown variant"));

        let decoded: CompanionRequest = serde_json::from_value(serde_json::json!({
            "op": "spawn_terminal_with_runtime",
            "program": "jcode",
            "args": [],
            "cwd": "/work",
            "runtime_env": [["JCODE_SOCKET", "/tmp/ok.sock"], ["JCODE_API_KEY", "drop"]]
        }))
        .unwrap();
        let CompanionRequest::SpawnTerminalWithRuntime { runtime_env, .. } = decoded else {
            panic!("decoded contextual request variant");
        };
        assert_eq!(
            runtime_env,
            [("JCODE_SOCKET".into(), "/tmp/ok.sock".into())]
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

#[cfg(test)]
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum OldCompanionRequest {
    Status,
    Open {
        target: String,
    },
    Reveal {
        path: PathBuf,
    },
    SpawnTerminal {
        program: PathBuf,
        args: Vec<String>,
        cwd: PathBuf,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        #[serde(default)]
        client_terminal_env: Vec<(String, String)>,
    },
    PowerActivity {
        active: bool,
    },
}
