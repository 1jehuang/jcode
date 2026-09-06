//! Scriptable SSH login transport. OAuth payloads only cross stdin, never argv.
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

const OUTPUT_LIMIT: usize = 64 * 1024;
pub(super) const INPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone)]
pub(super) struct Target {
    host: String,
    binary: String,
    cwd: Option<String>,
    socket: Option<String>,
}

impl Target {
    pub(super) fn from_env() -> Result<Self, &'static str> {
        let host = crate::tui::ssh_remote_host().ok_or("SSH host is not configured")?;
        let target = Self {
            host,
            binary: std::env::var("JCODE_SSH_BINARY").unwrap_or_else(|_| "jcode".into()),
            cwd: std::env::var("JCODE_SSH_WORKING_DIR").ok(),
            socket: std::env::var("JCODE_SSH_SERVER_SOCKET").ok(),
        };
        if target.host.starts_with('-')
            || target.host.chars().any(char::is_whitespace)
            || [&target.host, &target.binary]
                .into_iter()
                .chain(target.cwd.iter())
                .chain(target.socket.iter())
                .any(|value| value.is_empty() || value.chars().any(char::is_control))
        {
            return Err("Invalid SSH login configuration");
        }
        Ok(target)
    }

    fn command(&self, provider: &str, flow: &str, operation: Operation) -> tokio::process::Command {
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let socket = self
            .socket
            .as_deref()
            .map(|v| format!(" --socket {}", quote(v)))
            .unwrap_or_default();
        let cwd = self
            .cwd
            .as_deref()
            .map(|v| format!(" --cwd {}", quote(v)))
            .unwrap_or_default();
        let mut command = tokio::process::Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "PermitLocalCommand=no",
            "-o",
            "ForkAfterAuthentication=no",
            "-o",
            "StdinNull=no",
            "-o",
            "RemoteCommand=none",
            "-o",
            "SessionType=default",
            "-o",
            "ControlMaster=no",
            "-S",
            "none",
            "-o",
            "ConnectTimeout=20",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
        ]);
        command.arg("--").arg(&self.host).arg(format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {} --no-update --no-selfdev{socket}{cwd} login --provider {} --no-browser --json --flow-id {} {}",
            quote(&self.binary), quote(provider), quote(flow), operation.flag(),
        ));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Begin,
    Callback,
    Code,
    Complete,
    Cancel,
}
impl Operation {
    fn flag(self) -> &'static str {
        match self {
            Self::Begin => "--print-auth-url",
            Self::Callback => "--callback-url -",
            Self::Code => "--auth-code -",
            Self::Complete => "--complete",
            Self::Cancel => "--cancel",
        }
    }
}

pub(super) enum Reply {
    Pending {
        auth_url: String,
        input_kind: String,
        user_code: Option<String>,
    },
    Authenticated {
        validation_warning: bool,
    },
    Cancelled,
}

fn parse_reply(bytes: &[u8], operation: Operation, provider: &str) -> Result<Reply, &'static str> {
    // Deliberately do not deserialize/format remote error messages or arbitrary JSON.
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "Invalid remote login response. Update Jcode on the remote host.")?;
    if value["provider"].as_str() != Some(provider) {
        return Err("Remote login provider mismatch");
    }
    match (value["status"].as_str(), operation) {
        (Some("authenticated"), Operation::Callback | Operation::Code | Operation::Complete) => {
            Ok(Reply::Authenticated {
                validation_warning: false,
            })
        }
        (Some("cancelled"), Operation::Cancel) => Ok(Reply::Cancelled),
        (Some("pending"), Operation::Begin) => {
            let auth_url = value["auth_url"]
                .as_str()
                .ok_or("Missing remote authorization URL")?;
            let url = url::Url::parse(auth_url).map_err(|_| "Invalid remote authorization URL")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || auth_url.chars().any(char::is_control)
            {
                return Err("Remote authorization URL must be HTTPS");
            }
            let input_kind = value["input_kind"]
                .as_str()
                .ok_or("Missing remote login input kind")?;
            if !matches!(
                input_kind,
                "auth_code" | "callback_url" | "auth_code_or_callback_url" | "complete"
            ) {
                return Err("Unsupported remote login input kind");
            }
            let user_code = value["user_code"]
                .as_str()
                .filter(|s| {
                    s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
                .map(str::to_owned);
            Ok(Reply::Pending {
                auth_url: auth_url.into(),
                input_kind: input_kind.into(),
                user_code,
            })
        }
        _ => Err("Unexpected remote login response. Update Jcode on the remote host."),
    }
}

async fn execute(
    target: &Target,
    provider: &str,
    flow: &str,
    operation: Operation,
    payload: Option<String>,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<Reply, &'static str> {
    let mut child = target
        .command(provider, flow, operation)
        .spawn()
        .map_err(|_| "Could not start SSH login")?;
    let mut stdin = child.stdin.take().ok_or("SSH login stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("SSH login stdout unavailable")?;
    let timeout = if operation == Operation::Complete {
        Duration::from_secs(900)
    } else {
        Duration::from_secs(120)
    };
    let exchange = async {
        if let Some(payload) = payload {
            if payload.len() > INPUT_LIMIT {
                return Err("Login input is too long");
            }
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(|_| "Could not send remote login input")?;
        }
        stdin
            .shutdown()
            .await
            .map_err(|_| "Could not finish remote login input")?;
        drop(stdin);
        let mut bytes = Vec::new();
        stdout
            .take((OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Could not read remote login response")?;
        if bytes.len() > OUTPUT_LIMIT {
            return Err("Remote login response exceeded size limit");
        }
        let status = child
            .wait()
            .await
            .map_err(|_| "Could not wait for SSH login")?;
        let reply = parse_reply(&bytes, operation, provider);
        if !status.success() {
            // Tokens are saved and success is emitted before CLI validation.
            if matches!(reply, Ok(Reply::Authenticated { .. })) {
                return Ok(Reply::Authenticated {
                    validation_warning: true,
                });
            }
            return Err(
                "Remote login was rejected or SSH failed. Check the callback, remote Jcode version, and SSH access, then retry.",
            );
        }
        reply
    };
    let result = tokio::select! {
        _ = cancel => Err("cancelled"),
        result = tokio::time::timeout(timeout, exchange) => result.unwrap_or(Err("Remote login timed out")),
    };
    // Includes cancellation, limits and timeout: kill AND reap, not just drop a PID.
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

pub(super) struct Task {
    cancel: Option<oneshot::Sender<()>>,
    pub(super) reply: oneshot::Receiver<Result<Reply, &'static str>>,
}
impl Task {
    #[cfg(test)]
    pub(super) fn ready(result: Result<Reply, &'static str>) -> Self {
        let (sender, reply) = oneshot::channel();
        let _ = sender.send(result);
        Self {
            cancel: None,
            reply,
        }
    }
    pub(super) fn spawn(
        target: Target,
        provider: String,
        flow: String,
        operation: Operation,
        payload: Option<String>,
    ) -> Self {
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let (reply_tx, reply) = oneshot::channel();
        tokio::spawn(async move {
            let mut result = execute(
                &target,
                &provider,
                &flow,
                operation,
                payload,
                &mut cancel_rx,
            )
            .await;
            if matches!(result, Err("cancelled")) {
                let (_keepalive, mut never_cancel) = oneshot::channel();
                result = execute(
                    &target,
                    &provider,
                    &flow,
                    Operation::Cancel,
                    None,
                    &mut never_cancel,
                )
                .await;
            }
            let _ = reply_tx.send(result);
        });
        Self {
            cancel: Some(cancel_tx),
            reply,
        }
    }
    pub(super) fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_login_command_quotes_paths_and_never_carries_payload() {
        let target = Target {
            host: "test-host".into(),
            binary: "/srv/a'b/jcode".into(),
            cwd: Some("/srv/a b".into()),
            socket: Some("/run/remote.sock".into()),
        };
        let cmd = target.command("openai", "random_flow", Operation::Callback);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let remote = args.last().unwrap();
        assert!(remote.contains("'/srv/a'\\''b/jcode'"));
        assert!(remote.contains("--cwd '/srv/a b'"));
        assert!(remote.contains("--socket '/run/remote.sock'"));
        assert!(remote.ends_with("--callback-url -"));
        assert!(remote.contains("--flow-id 'random_flow'"));
        assert!(args.iter().any(|a| a == "StrictHostKeyChecking=yes"));
    }
    #[test]
    fn ssh_login_json_only_accepts_expected_safe_fields() {
        let response = br#"{"status":"pending","provider":"openai","auth_url":"https://auth.openai.com/authorize?state=x","input_kind":"callback_url","verifier":"must-not-surface"}"#;
        assert!(matches!(
            parse_reply(response, Operation::Begin, "openai"),
            Ok(Reply::Pending { .. })
        ));
        assert!(parse_reply(response, Operation::Begin, "claude").is_err());
        let unsafe_url = br#"{"status":"pending","provider":"openai","auth_url":"file:///etc/passwd","input_kind":"callback_url"}"#;
        assert!(parse_reply(unsafe_url, Operation::Begin, "openai").is_err());
        let error = br#"{"status":"error","provider":"openai","message":"secret-code"}"#;
        assert!(
            !parse_reply(error, Operation::Callback, "openai")
                .err()
                .unwrap()
                .contains("secret-code")
        );
    }
}
