//! Scriptable SSH login transport. Secret payloads only cross stdin, never argv.
use crate::auth::transfer::{
    CredentialTransfer, MAX_TRANSFER_BYTES, TransferProvider, export_local,
};
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
    pub(super) fn host(&self) -> &str {
        &self.host
    }

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
        let action = if operation == Operation::Import {
            format!("auth import --provider {} --stdin --json", quote(provider))
        } else {
            format!(
                "login --provider {} --no-browser --json --flow-id {} {}",
                quote(provider),
                quote(flow),
                operation.flag(),
            )
        };
        command.arg("--").arg(&self.host).arg(format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {} --no-update --no-selfdev{socket}{cwd} {action}",
            quote(&self.binary),
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
    Import,
}
impl Operation {
    fn flag(self) -> &'static str {
        match self {
            Self::Begin => "--print-auth-url",
            Self::Callback => "--callback-url -",
            Self::Code => "--auth-code -",
            Self::Complete => "--complete",
            Self::Cancel => "--cancel",
            Self::Import => unreachable!("credential import does not use OAuth flags"),
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
    Imported,
}

fn parse_reply(bytes: &[u8], operation: Operation, provider: &str) -> Result<Reply, &'static str> {
    // Deliberately do not deserialize/format remote error messages or arbitrary JSON.
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "Invalid remote login response. Update Jcode on the remote host.")?;
    if value["provider"].as_str() != Some(provider) {
        return Err("Remote login provider mismatch");
    }
    match (value["status"].as_str(), operation) {
        (Some("imported"), Operation::Import) => Ok(Reply::Imported),
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

// Not Debug or Clone. Keep the base export's private buffer alive only for stdin.
enum Payload {
    Login(String),
    Import(CredentialTransfer),
}

impl Payload {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Login(value) => value.as_bytes(),
            Self::Import(value) => value.as_bytes(),
        }
    }
}

async fn execute(
    target: &Target,
    provider: &str,
    flow: &str,
    operation: Operation,
    payload: Option<Payload>,
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
            let limit = if operation == Operation::Import {
                MAX_TRANSFER_BYTES
            } else {
                INPUT_LIMIT
            };
            if payload.as_bytes().len() > limit {
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
            if operation == Operation::Import {
                return Err(
                    "Remote credential import was rejected or SSH failed. Existing remote credentials are never overwritten. Check remote Jcode version and SSH access.",
                );
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

pub(super) fn cleanup_detached(target: Target, provider: String, flow: String) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            let (_keepalive, mut never_cancel) = oneshot::channel();
            let _ = execute(
                &target,
                &provider,
                &flow,
                Operation::Cancel,
                None,
                &mut never_cancel,
            )
            .await;
        });
    }
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
            // This operation is created only by the explicit private confirmation
            // handler, never by startup, provider discovery, or the login picker.
            let payload = if operation == Operation::Import {
                if !matches!(
                    cancel_rx.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ) {
                    let _ = reply_tx.send(Err("cancelled"));
                    return;
                }
                let exported = provider
                    .parse::<TransferProvider>()
                    .ok()
                    .and_then(|provider| export_local(provider).ok());
                let Some(exported) = exported else {
                    let _ = reply_tx.send(Err("Could not export the selected local account. No credentials were sent. Check the local provider login and retry with explicit confirmation."));
                    return;
                };
                Some(Payload::Import(exported))
            } else {
                payload.map(Payload::Login)
            };
            let mut result = execute(
                &target,
                &provider,
                &flow,
                operation,
                payload,
                &mut cancel_rx,
            )
            .await;
            if matches!(result, Err("cancelled")) && operation != Operation::Import {
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

    #[test]
    fn ssh_import_reuses_target_and_hardening_without_oauth_or_secret_arguments() {
        let target = Target {
            host: "test-host".into(),
            binary: "/srv/a'b/jcode".into(),
            cwd: Some("/srv/a b".into()),
            socket: Some("/run/remote.sock".into()),
        };
        for provider in ["openai", "claude"] {
            let cmd = target.command(provider, "unused-flow", Operation::Import);
            let args: Vec<_> = cmd
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let remote = args.last().unwrap();
            assert!(remote.contains("'/srv/a'\\''b/jcode'"));
            assert!(remote.contains("--cwd '/srv/a b'"));
            assert!(remote.contains("--socket '/run/remote.sock'"));
            assert!(remote.ends_with(&format!(
                "auth import --provider '{provider}' --stdin --json"
            )));
            assert!(!remote.contains("--flow-id"));
            assert!(!remote.contains("unused-flow"));
            assert!(!remote.contains("--callback-url"));
            for required in [
                "-T",
                "BatchMode=yes",
                "StrictHostKeyChecking=yes",
                "ForwardAgent=no",
                "ClearAllForwardings=yes",
                "PermitLocalCommand=no",
                "StdinNull=no",
                "RemoteCommand=none",
                "ControlMaster=no",
                "ConnectTimeout=20",
            ] {
                assert!(args.iter().any(|arg| arg == required), "{required}");
            }
        }
    }

    #[test]
    fn ssh_import_accepts_only_matching_import_success_and_ignores_remote_error_text() {
        let imported = br#"{"status":"imported","provider":"openai","secret":"must-not-surface"}"#;
        assert!(matches!(
            parse_reply(imported, Operation::Import, "openai"),
            Ok(Reply::Imported)
        ));
        assert!(parse_reply(imported, Operation::Import, "claude").is_err());
        assert!(parse_reply(imported, Operation::Begin, "openai").is_err());
        for response in [
            br#"{"status":"authenticated","provider":"openai"}"#.as_slice(),
            br#"{"status":"error","provider":"openai","message":"must-not-surface"}"#.as_slice(),
            b"not-json must-not-surface".as_slice(),
        ] {
            let error = parse_reply(response, Operation::Import, "openai")
                .err()
                .unwrap();
            assert!(!error.contains("must-not-surface"));
        }
    }
}
