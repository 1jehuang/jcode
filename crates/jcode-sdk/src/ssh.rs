//! System OpenSSH transport. Authentication and host aliases come from the user's SSH setup.

use crate::{ConnectOptions, Error, ErrorKind, Result, Transport};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Connect to the user's persistent native harness on an SSH host.
///
/// The remote installation must support `jcode api --stdio`. No software is
/// installed or updated. Unknown host keys must first be verified using SSH.
/// The remote login shell must accept POSIX shell syntax. Authentication uses
/// system SSH config, keys and agent, without password/host-key prompts. SSH
/// forwarding, connection multiplexing and configured local commands are disabled
/// so the SDK owns exactly one foreground SSH connection.
///
/// ```no_run
/// use jcode_sdk::{JcodeClient, SshConnectOptions};
/// let client = JcodeClient::connect_ssh(SshConnectOptions::new("desktop"))?;
/// let sessions = client.list_sessions()?;
/// // Dropping the last clone closes SSH, not the remote shared daemon.
/// drop(client);
/// # Ok::<(), jcode_sdk::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct SshConnectOptions {
    /// SSH config alias, hostname, IP address, or `user@host`. Not a shell command.
    pub host: String,
    /// Override the port from SSH config.
    pub port: Option<u16>,
    /// Override the user from SSH config. Do not also put a user in `host`.
    pub user: Option<String>,
    /// Remote executable name or literal path (no shell expansion).
    pub remote_binary: String,
    /// Total SSH startup and API handshake deadline, independent of request_timeout.
    pub connect_timeout: Duration,
    pub client_name: String,
    pub request_timeout: Option<Duration>,
}

impl Default for SshConnectOptions {
    fn default() -> Self {
        let defaults = ConnectOptions::default();
        Self {
            host: String::new(),
            port: None,
            user: None,
            remote_binary: "jcode".into(),
            connect_timeout: Duration::from_secs(30),
            client_name: defaults.client_name,
            request_timeout: defaults.request_timeout,
        }
    }
}

impl SshConnectOptions {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            ..Self::default()
        }
    }

    fn validate(&self) -> Result<()> {
        let invalid = |message| Error::new(ErrorKind::InvalidOption, message);
        // OpenSSH may interpolate %h/%r into configured ProxyCommand shells.
        // Validate even though Command itself does not invoke a local shell.
        let valid_user = |s: &str| {
            !s.is_empty()
                && !s.starts_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        };
        let (user, host) = match self.host.split_once('@') {
            Some((user, host)) if valid_user(user) && self.user.is_none() => (Some(user), host),
            Some(_) => {
                return Err(invalid(
                    "SSH host must be a hostname or user@hostname; do not specify the user twice",
                ));
            }
            None => (None, self.host.as_str()),
        };
        let _ = user;
        if host.is_empty()
            || host.starts_with('-')
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-:[]%".contains(&b))
        {
            return Err(invalid(
                "invalid SSH host: use an SSH alias, hostname, or IP address, not options or a command",
            ));
        }
        if self.user.as_deref().is_some_and(|s| !valid_user(s)) {
            return Err(invalid("invalid SSH user"));
        }
        if self.port == Some(0) {
            return Err(invalid("SSH port must be between 1 and 65535"));
        }
        if self.remote_binary.is_empty()
            || self.remote_binary.starts_with('-')
            || self.remote_binary.chars().any(char::is_control)
        {
            return Err(invalid(
                "remote_binary must be an executable name or literal path without control characters",
            ));
        }
        if self.connect_timeout.is_zero() || self.connect_timeout > Duration::from_secs(3600) {
            return Err(invalid(
                "SSH connect_timeout must be greater than zero and at most one hour",
            ));
        }
        // Keep the first frame below pipe capacity so even a stalled SSH
        // process cannot block the hello write before its timed reply wait.
        if self.client_name.len() > 1024
            || serde_json::to_string(&self.client_name).map_or(true, |s| s.len() > 2048)
        {
            return Err(invalid("SSH client_name must be at most 1024 bytes"));
        }
        Ok(())
    }

    pub(crate) fn command(&self) -> Result<Command> {
        self.validate()?;
        let mut command = Command::new("ssh");
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
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
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
        ]);
        command.arg("-o").arg(format!(
            "ConnectTimeout={}",
            self.connect_timeout.as_secs().max(1)
        ));
        if let Some(port) = self.port {
            command.arg("-p").arg(port.to_string());
        }
        if let Some(user) = &self.user {
            command.arg("-l").arg(user);
        }
        // SSH joins remote arguments into shell source. Supply exactly one,
        // quoting the executable as a literal POSIX shell word.
        let remote = format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {} --no-update api --stdio",
            shell_quote(&self.remote_binary)
        );
        command.arg("--").arg(&self.host).arg(remote);
        Ok(command)
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

const STDERR_LIMIT: usize = 16 * 1024;

#[cfg(all(test, unix))]
#[path = "ssh_integration_tests.rs"]
mod integration_tests;

pub(crate) struct SshProcess {
    pub(crate) timed_out: AtomicBool,
    child: Mutex<Option<Child>>,
    status: Mutex<Option<std::process::ExitStatus>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_done: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl SshProcess {
    pub(crate) fn startup_deadline(
        self: &Arc<Self>,
        timeout: Duration,
    ) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let (cancel, cancelled) = std::sync::mpsc::channel();
        let process = Arc::clone(self);
        let watchdog = std::thread::spawn(move || {
            if matches!(
                cancelled.recv_timeout(timeout),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                process.timed_out.store(true, Ordering::Release);
                process.shutdown();
            }
        });
        (cancel, watchdog)
    }

    pub(crate) fn shutdown(&self) {
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                // A dedicated process group also closes ProxyCommand helpers.
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                if let Ok(status) = child.wait() {
                    if let Ok(mut saved) = self.status.lock() {
                        *saved = Some(status);
                    }
                }
            }
        }
        // Usually EOF arrives immediately. Never hang cleanup on an inherited
        // stderr handle held by a configured external SSH helper.
        if let Ok(mut done) = self.stderr_done.lock() {
            if let Some(done) = done.take() {
                let _ = done.recv_timeout(Duration::from_millis(100));
            }
        }
    }

    pub(crate) fn diagnostic(&self) -> String {
        let detail = self
            .stderr
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default();
        let status = self
            .status
            .lock()
            .ok()
            .and_then(|s| *s)
            .map(|s| format!(" ({s})"))
            .unwrap_or_default();
        format!(
            "SSH harness connection closed{status}. Check SSH authentication and known_hosts, and that remote jcode supports `api --stdio`.{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(" SSH stderr: {detail}")
            }
        )
    }
}

impl Drop for SshProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) struct SshTransport {
    pub(crate) process: Arc<SshProcess>,
    reader: Option<std::process::ChildStdout>,
    writer: Option<std::process::ChildStdin>,
}

impl SshTransport {
    pub(crate) fn spawn(options: &SshConnectOptions) -> Result<Self> {
        Self::spawn_command(options.command()?)
    }

    pub(crate) fn spawn_command(mut command: Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                Error::new(
                    ErrorKind::ConnectFailed,
                    format!("could not start system ssh: {error}"),
                )
            })?;
        let reader = child.stdout.take();
        let writer = child.stdin.take();
        let mut stderr_pipe = child.stderr.take().expect("piped stderr");
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let buffer = Arc::clone(&stderr);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            loop {
                match stderr_pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut bytes) = buffer.lock() {
                            bytes.extend_from_slice(&chunk[..n]);
                            if bytes.len() > STDERR_LIMIT {
                                let excess = bytes.len() - STDERR_LIMIT;
                                bytes.drain(..excess);
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = done_tx.send(());
        });
        let process = Arc::new(SshProcess {
            timed_out: AtomicBool::new(false),
            child: Mutex::new(Some(child)),
            status: Mutex::new(None),
            stderr,
            stderr_done: Mutex::new(Some(done_rx)),
        });
        Ok(Self {
            process,
            reader,
            writer,
        })
    }
}

impl Transport for SshTransport {
    fn shutdown_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let process = Arc::clone(&self.process);
        Some(Arc::new(move || process.shutdown()))
    }

    fn split(mut self: Box<Self>) -> Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        Ok((
            Box::new(BufReader::new(self.reader.take().expect("SSH stdout"))),
            Box::new(self.writer.take().expect("SSH stdin")),
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        JcodeClient,
        api::{API_VERSION_MAJOR, ApiEvent, ServerFrame},
    };
    use std::time::Instant;

    fn fake(script: &str) -> (SshTransport, u32) {
        let mut command = Command::new("/bin/sh");
        command.env_clear().args(["-c", script]);
        let transport = SshTransport::spawn_command(command).unwrap();
        let pid = transport
            .process
            .child
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .id();
        (transport, pid)
    }

    fn options() -> SshConnectOptions {
        SshConnectOptions {
            connect_timeout: Duration::from_millis(150),
            request_timeout: None,
            ..SshConnectOptions::new("fake-host")
        }
    }

    fn assert_reaped(pid: u32) {
        let result = unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
        assert_eq!(result, -1, "SSH child was not reaped");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    fn hello() -> String {
        serde_json::to_string(&ServerFrame::reply(
            1,
            ApiEvent::HelloOk {
                version: API_VERSION_MAJOR,
                server: "fake-ssh".into(),
                capabilities: vec![],
            },
        ))
        .unwrap()
    }

    #[test]
    fn rejects_hosts_that_can_inject_options_or_shell_commands() {
        for host in [
            "",
            "-oProxyCommand=evil",
            "host name",
            "host;touch x",
            "$(evil)",
            "x`evil`",
            "x\ny",
            "ssh://host",
            "a@b@c",
            "-user@host",
        ] {
            assert!(
                SshConnectOptions::new(host).command().is_err(),
                "accepted {host:?}"
            );
        }
        for host in [
            "desktop",
            "user@host.example",
            "127.0.0.1",
            "[::1]",
            "fe80::1%eth0",
        ] {
            assert!(
                SshConnectOptions::new(host).command().is_ok(),
                "rejected {host}"
            );
        }
        let mut opts = options();
        opts.user = Some("root;evil".into());
        assert!(opts.command().is_err());
        opts.user = None;
        opts.port = Some(0);
        assert!(opts.command().is_err());
        opts.port = None;
        opts.connect_timeout = Duration::ZERO;
        assert!(opts.command().is_err());
        opts.connect_timeout = Duration::from_secs(2);
        opts.client_name = "\0".repeat(1024);
        assert!(opts.command().is_err());
    }

    #[test]
    fn preserves_system_ssh_configuration_without_weakening_host_verification() {
        let mut opts = options();
        opts.port = Some(2222);
        opts.user = Some("alice".into());
        opts.remote_binary = "/opt/a b/jcode'$(false)".into();
        let command = opts.command().unwrap();
        let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(command.get_program(), "ssh");
        assert!(args.contains(&"StrictHostKeyChecking=yes"));
        assert!(args.contains(&"BatchMode=yes"));
        assert!(!args.contains(&"-F"));
        assert!(!args.iter().any(|arg| arg.contains("KnownHostsFile")));
        assert!(args.windows(2).any(|a| a == ["-p", "2222"]));
        assert!(args.windows(2).any(|a| a == ["-l", "alice"]));
        assert_eq!(args[args.len() - 2], "fake-host");
        assert!(args.last().unwrap().ends_with(" --no-update api --stdio"));
        let quoted = shell_quote(&opts.remote_binary);
        let output = Command::new("/bin/sh")
            .args(["-c", &format!("printf '%s' {quoted}")])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            opts.remote_binary
        );
    }

    #[test]
    fn handshake_timeout_is_bounded_even_without_request_timeout_and_reaps_child() {
        let (transport, pid) = fake("echo 'waiting for remote jcode' >&2; exec /bin/sleep 30");
        let start = Instant::now();
        let error = JcodeClient::over_ssh(transport, options()).err().unwrap();
        assert_eq!(error.kind, ErrorKind::StartupTimeout);
        assert!(error.message.contains("waiting for remote jcode"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_reaped(pid);
    }

    #[test]
    fn ssh_stderr_is_reported_and_failed_child_is_reaped() {
        let (transport, pid) = fake("echo 'Permission denied (publickey)' >&2; exit 255");
        let error = JcodeClient::over_ssh(transport, options()).err().unwrap();
        assert!(
            error.message.contains("Permission denied (publickey)"),
            "{error}"
        );
        assert!(error.message.contains("api --stdio"));
        assert_reaped(pid);
    }

    #[test]
    fn final_clone_drop_kills_and_reaps_but_earlier_clone_drop_does_not() {
        let pong = serde_json::to_string(&ServerFrame::reply(2, ApiEvent::Pong)).unwrap();
        let script = format!(
            "read -r hello; printf '%s\\n' {}; read -r ping; printf '%s\\n' {}; exec /bin/sleep 30",
            shell_quote(&hello()),
            shell_quote(&pong)
        );
        let (transport, pid) = fake(&script);
        let client = JcodeClient::over_ssh(transport, options()).unwrap();
        assert!(client.socket_path().as_os_str().is_empty());
        let clone = client.clone();
        drop(client);
        clone.ping().unwrap();
        let start = Instant::now();
        drop(clone);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_reaped(pid);
    }

    #[test]
    fn malformed_handshake_closes_owned_ssh_process() {
        let (transport, pid) = fake("read -r hello; echo 'not JSON'; exec /bin/sleep 30");
        assert!(JcodeClient::over_ssh(transport, options()).is_err());
        assert_reaped(pid);
    }

    #[test]
    fn dropping_unconnected_transport_reaps_ssh() {
        let (transport, pid) = fake("exec /bin/sleep 30");
        drop(transport);
        assert_reaped(pid);
    }

    #[test]
    fn startup_watchdog_interrupts_a_blocked_hello_write() {
        let (transport, pid) = fake("exec /bin/sleep 30");
        let mut opts = options();
        // Bypass public validation to exercise the process watchdog, rather
        // than depending on a platform's pipe capacity for startup safety.
        opts.client_name = "x".repeat(1024 * 1024);
        let start = Instant::now();
        let error = JcodeClient::over_ssh(transport, opts).err().unwrap();
        assert_eq!(error.kind, ErrorKind::StartupTimeout);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_reaped(pid);
    }

    #[test]
    fn established_disconnect_fails_requests_with_ssh_stderr_and_reaps() {
        let script = format!(
            "read -r hello; printf '%s\\n' {}; read -r ping; echo 'remote channel failed' >&2; exit 17",
            shell_quote(&hello())
        );
        let (transport, pid) = fake(&script);
        let client = JcodeClient::over_ssh(transport, options()).unwrap();
        let error = client.ping().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Disconnected);
        assert!(error.message.contains("remote channel failed"), "{error}");
        assert!(client.is_closed());
        assert_reaped(pid);
    }

    #[test]
    fn streaming_disconnect_preserves_ssh_stderr() {
        let script = format!(
            "read -r hello; printf '%s\\n' {}; read -r message; echo 'stream channel failed' >&2; exit 17",
            shell_quote(&hello())
        );
        let (transport, pid) = fake(&script);
        let client = JcodeClient::over_ssh(transport, options()).unwrap();
        let error = client
            .run("fake-session", "test", crate::RunOptions::default())
            .unwrap_err();
        assert!(error.message.contains("stream channel failed"), "{error}");
        assert_reaped(pid);
    }

    #[test]
    fn stderr_is_bounded_and_drained_without_backpressure() {
        let script = "i=0; while [ $i -lt 5000 ]; do echo 'a fairly long SSH diagnostic line' >&2; i=$((i+1)); done; echo FINAL_DIAGNOSTIC >&2; exit 255";
        let (transport, pid) = fake(script);
        let mut opts = options();
        opts.connect_timeout = Duration::from_secs(5);
        let error = JcodeClient::over_ssh(transport, opts).err().unwrap();
        assert!(error.message.contains("FINAL_DIAGNOSTIC"));
        assert!(error.message.len() < STDERR_LIMIT + 1024);
        assert_reaped(pid);
    }
}
