//! Native TUI transport over owned, non-interactive OpenSSH connections.
//!
//! The private local socket is only an adapter. Each connection carries the
//! native Request/ServerEvent protocol, not the SDK harness API. Closing it
//! closes SSH and its bridge, never the remote shared daemon.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

const PROTOCOL: u32 = 1;
const HANDSHAKE_LIMIT: usize = 8192;
const STDERR_LIMIT: usize = 16 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONNECTIONS: usize = 32;

fn private_directory() -> Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(0o700);
    // tempfile's directory default is 0777 subject to umask, unlike its file
    // default. Set the creation mode, so there is no permissive chmod window.
    let directory = tempfile::Builder::new()
        .prefix("jcode-ssh-")
        .permissions(permissions.clone())
        .tempdir()?;
    // Restore owner access even under an unusually restrictive owner umask.
    std::fs::set_permissions(directory.path(), permissions)?;
    Ok(directory)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NativeHandshake {
    pub kind: String,
    pub protocol: u32,
    pub version: String,
    pub working_dir: String,
    pub socket_path: String,
}

/// Lifetime guard for the private socket and all its SSH children.
pub(crate) struct NativeSsh {
    _directory: tempfile::TempDir,
    socket: PathBuf,
    host: String,
    handshake: NativeHandshake,
    stop: watch::Sender<bool>,
    manager: Option<JoinHandle<()>>,
}

impl NativeSsh {
    pub async fn connect_with_workspace(
        host: &str,
        remote_binary: &str,
        daemon_socket: Option<&str>,
        remote_working_dir: Option<&str>,
    ) -> Result<Self> {
        let mut options = SshOptions::new(host, remote_binary)?;
        if daemon_socket
            .is_some_and(|socket| socket.is_empty() || socket.chars().any(char::is_control))
        {
            bail!(
                "remote daemon socket must be a literal nonempty path without control characters"
            );
        }
        options.daemon_socket = daemon_socket.map(str::to_owned);
        if remote_working_dir
            .is_some_and(|path| path.is_empty() || path.chars().any(char::is_control))
        {
            bail!(
                "remote working directory must be a literal nonempty path without control characters"
            );
        }
        options.working_dir = remote_working_dir.map(str::to_owned);
        // Fail before entering the TUI, with authentication/host-key diagnostics.
        let mut probe = SshConnection::connect(&options).await?;
        let handshake = probe.handshake.clone();
        probe.shutdown().await;
        // Remote --cwd already verified the directory and the header contains
        // its resolved path. Keep reconnects on that same verified workspace.
        options.working_dir = Some(handshake.working_dir.clone());

        let directory = private_directory()?;
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket)?;
        let (stop, stopped) = watch::channel(false);
        let manager = tokio::spawn(accept_connections(listener, options, stopped));
        Ok(Self {
            _directory: directory,
            socket,
            host: host.into(),
            handshake,
            stop,
            manager: Some(manager),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn handshake(&self) -> &NativeHandshake {
        &self.handshake
    }
    pub fn remote_working_dir(&self) -> &str {
        &self.handshake.working_dir
    }

    /// Close and reap owned SSH children before the Tokio runtime shuts down.
    /// Keep the guard outside a signal/TUI select and await this on either exit.
    pub async fn close(&mut self) -> Result<()> {
        let _ = self.stop.send(true);
        let _ = std::fs::remove_file(&self.socket);
        let Some(mut manager) = self.manager.take() else {
            return Ok(());
        };
        match tokio::time::timeout(Duration::from_secs(5), &mut manager).await {
            Ok(result) => result.context("native SSH cleanup task failed"),
            Err(_) => {
                // Dropping the manager's JoinSet drops every owned-child guard.
                manager.abort();
                let _ = manager.await;
                bail!("native SSH cleanup timed out; owned child tasks were aborted")
            }
        }
    }
}

impl Drop for NativeSsh {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        // Remove the address immediately so nobody can dial after guard drop.
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[derive(Clone)]
struct SshOptions {
    host: String,
    remote_binary: String,
    daemon_socket: Option<String>,
    working_dir: Option<String>,
}

impl SshOptions {
    fn new(host: &str, remote_binary: &str) -> Result<Self> {
        let valid_user = |s: &str| {
            !s.is_empty()
                && !s.starts_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        };
        let hostname = match host.split_once('@') {
            Some((user, hostname)) if valid_user(user) => hostname,
            Some(_) => bail!("invalid SSH user in host"),
            None => host,
        };
        if hostname.is_empty()
            || hostname.starts_with('-')
            || !hostname
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-:[]%".contains(&b))
        {
            bail!("invalid SSH host: use a configured alias, hostname, or user@host");
        }
        if remote_binary.is_empty()
            || remote_binary.starts_with('-')
            || remote_binary.chars().any(char::is_control)
        {
            bail!(
                "remote binary must be an executable name or literal path without control characters"
            );
        }
        Ok(Self {
            host: host.into(),
            remote_binary: remote_binary.into(),
            daemon_socket: None,
            working_dir: None,
        })
    }

    fn command(&self) -> Command {
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
            "-o",
            "ConnectTimeout=30",
        ]);
        let binary = format!("'{}'", self.remote_binary.replace('\'', "'\\''"));
        let socket = self
            .daemon_socket
            .as_ref()
            .map(|socket| format!(" --socket '{}'", socket.replace('\'', "'\\''")))
            .unwrap_or_default();
        let cwd = self
            .working_dir
            .as_ref()
            .map(|path| format!(" --cwd '{}'", path.replace('\'', "'\\''")))
            .unwrap_or_default();
        let remote = format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {binary} --no-update --no-selfdev{socket}{cwd} server stdio"
        );
        command.arg("--").arg(&self.host).arg(remote);
        command
    }
}

struct OwnedChild(Child);
impl OwnedChild {
    fn kill(&mut self) {
        if let Some(pid) = self.0.id() {
            // Only the dedicated group we created, including ProxyCommand helpers.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = self.0.start_kill();
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill();
    }
}

struct SshConnection {
    child: OwnedChild,
    reader: BufReader<ChildStdout>,
    writer: ChildStdin,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
    handshake: NativeHandshake,
}

impl SshConnection {
    async fn connect(options: &SshOptions) -> Result<Self> {
        Self::spawn(options.command(), STARTUP_TIMEOUT)
            .await
            .with_context(|| format!("connecting native Jcode on {}", options.host))
    }

    async fn spawn(command: Command, deadline: Duration) -> Result<Self> {
        let mut connection = Self::spawn_process(command)?;
        if let Err(error) = connection.establish(deadline).await {
            connection.shutdown().await;
            return Err(error.context(connection.diagnostic()));
        }
        Ok(connection)
    }

    fn spawn_process(mut command: Command) -> Result<Self> {
        command.as_std_mut().process_group(0);
        #[cfg(target_os = "linux")]
        {
            let parent_pid = std::process::id();
            // SIGKILL/process::exit do not run guards. Do not leave SSH holding
            // a remote bridge open if the local UI disappears abruptly.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent_pid {
                        libc::raise(libc::SIGKILL);
                    }
                    Ok(())
                });
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = OwnedChild(command.spawn().context("starting system ssh")?);
        let reader = BufReader::new(child.0.stdout.take().context("SSH stdout missing")?);
        let writer = child.0.stdin.take().context("SSH stdin missing")?;
        let mut stderr_pipe = child.0.stderr.take().context("SSH stderr missing")?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let buffer = Arc::clone(&stderr);
        let stderr_task = tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr_pipe.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut bytes) = buffer.lock() {
                    bytes.extend_from_slice(&chunk[..n]);
                    if bytes.len() > STDERR_LIMIT {
                        let excess = bytes.len() - STDERR_LIMIT;
                        bytes.drain(..excess);
                    }
                }
            }
        });
        Ok(Self {
            child,
            reader,
            writer,
            stderr,
            stderr_task,
            handshake: NativeHandshake {
                kind: String::new(),
                protocol: 0,
                version: String::new(),
                working_dir: String::new(),
                socket_path: String::new(),
            },
        })
    }

    async fn establish(&mut self, deadline: Duration) -> Result<()> {
        self.handshake = tokio::time::timeout(deadline, read_handshake(&mut self.reader))
            .await
            .context("SSH startup/handshake timed out")??;
        Ok(())
    }

    fn diagnostic(&self) -> String {
        let stderr = self
            .stderr
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
            .unwrap_or_default();
        format!(
            "Native SSH connection closed. Verify SSH credentials/known_hosts and remote `jcode server stdio` support. {stderr}"
        )
    }

    async fn shutdown(&mut self) {
        self.child.kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.0.wait()).await;
        if tokio::time::timeout(Duration::from_millis(100), &mut self.stderr_task)
            .await
            .is_err()
        {
            self.stderr_task.abort();
        }
    }
}

impl Drop for SshConnection {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}

async fn read_handshake<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<NativeHandshake> {
    let frame = read_bounded_line(reader).await?;
    let handshake: NativeHandshake =
        serde_json::from_slice(&frame).context("invalid native SSH handshake JSON")?;
    if handshake.kind != "jcode-native-stdio" || handshake.protocol != PROTOCOL {
        bail!(
            "unsupported native SSH protocol {} ({})",
            handshake.protocol,
            handshake.kind
        );
    }
    if handshake.version.is_empty()
        || handshake.socket_path.is_empty()
        || handshake.working_dir.is_empty()
    {
        bail!("incomplete native SSH handshake metadata");
    }
    Ok(handshake)
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            bail!("remote closed before native SSH handshake");
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let count = newline.map_or(available.len(), |n| n + 1);
        if frame.len() + count > HANDSHAKE_LIMIT {
            bail!("native SSH handshake exceeds {HANDSHAKE_LIMIT} bytes");
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    Ok(frame)
}

async fn verify_daemon_protocol<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    tokio::time::timeout(STARTUP_TIMEOUT, async {
        writer.write_all(b"{\"type\":\"ping\",\"id\":0}\n").await?;
        writer.flush().await?;
        let frame = read_bounded_line(reader).await?;
        let pong: serde_json::Value = serde_json::from_slice(&frame)?;
        if pong["type"] != "pong" || pong["id"].as_u64() != Some(0)
            || pong["native_ssh_protocol"].as_u64() != Some(u64::from(PROTOCOL)) {
            bail!("remote daemon does not support native SSH protocol {PROTOCOL}; update/reload the remote Jcode server or select a matching daemon socket");
        }
        Ok::<_, anyhow::Error>(())
    }).await.context("remote daemon native SSH capability handshake timed out")??;
    Ok(())
}

async fn accept_connections(
    listener: UnixListener,
    options: SshOptions,
    mut stopped: watch::Receiver<bool>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stopped.changed() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { break; };
                if connections.len() >= MAX_CONNECTIONS { continue; }
                let options = options.clone();
                let mut stopped = stopped.clone();
                connections.spawn(async move {
                    let mut ssh = match SshConnection::spawn_process(options.command()) {
                        Ok(ssh) => ssh,
                        Err(error) => { crate::logging::warn(&format!("Native SSH reconnect failed: {error:#}")); return; }
                    };
                    let connected = tokio::select! {
                        biased;
                        _ = stopped.changed() => None,
                        connected = ssh.establish(STARTUP_TIMEOUT) => Some(connected),
                    };
                    if !matches!(connected, Some(Ok(()))) {
                        ssh.shutdown().await;
                        if let Some(Err(error)) = connected {
                            crate::logging::warn(&format!("Native SSH reconnect failed: {error:#}; {}", ssh.diagnostic()));
                        }
                        return;
                    }
                    let (mut read, mut write) = stream.into_split();
                    let outcome = tokio::select! {
                        biased;
                        _ = stopped.changed() => Ok(0),
                        result = tokio::io::copy(&mut read, &mut ssh.writer) => result,
                        result = tokio::io::copy(&mut ssh.reader, &mut write) => result,
                    };
                    ssh.shutdown().await;
                    if let Err(error) = outcome {
                        crate::logging::warn(&format!("Native SSH stream failed: {error}; {}", ssh.diagnostic()));
                    }
                });
            }
        }
    }
    drop(listener);
    // Guard drop wakes all children, which explicitly kill and reap SSH.
    while connections.join_next().await.is_some() {}
}

/// CLI-only stdin/stdout bridge to an already-running persistent native server.
pub(crate) async fn run_stdio(socket: PathBuf) -> Result<()> {
    use std::io::Write;
    // Tokio stdin is uncancellable and can keep runtime shutdown alive after
    // daemon death while SSH still holds stdin open. A plain thread cannot.
    let (input, mut writer) = std::os::unix::net::UnixStream::pair()?;
    input.set_nonblocking(true)?;
    let input = UnixStream::from_std(input)?;
    std::thread::Builder::new()
        .name("native-ssh-stdin".into())
        .spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut writer);
            let _ = writer.flush();
            let _ = writer.shutdown(std::net::Shutdown::Write);
        })?;
    bridge_stream(input, tokio::io::stdout(), socket).await
}

async fn bridge_stream<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: R,
    mut output: W,
    socket: PathBuf,
) -> Result<()> {
    let daemon = tokio::time::timeout(STARTUP_TIMEOUT, UnixStream::connect(&socket))
        .await
        .context("native daemon connection timed out")??;
    let (read, mut write) = daemon.into_split();
    let mut read = BufReader::new(read);
    verify_daemon_protocol(&mut read, &mut write).await?;
    let handshake = NativeHandshake {
        kind: "jcode-native-stdio".into(),
        protocol: PROTOCOL,
        version: jcode_build_meta::version().to_string(),
        working_dir: std::env::current_dir()?.to_string_lossy().into_owned(),
        socket_path: socket.to_string_lossy().into_owned(),
    };
    let mut header = serde_json::to_vec(&handshake)?;
    header.push(b'\n');
    if header.len() > HANDSHAKE_LIMIT {
        bail!("native SSH handshake metadata too large");
    }
    output.write_all(&header).await?;
    output.flush().await?;
    {
        let upload = async {
            tokio::io::copy(&mut input, &mut write).await?;
            // A shell pipeline closes stdin after its last request. Forward
            // that half-close but do not discard replies already in flight.
            write.shutdown().await
        };
        let download = tokio::io::copy(&mut read, &mut output);
        tokio::pin!(upload, download);
        tokio::select! {
            result = &mut upload => {
                result?;
                // Detaching a subscribed client must not wait for a remote
                // model turn. Drain final protocol replies for a bounded time.
                if let Ok(result) = tokio::time::timeout(Duration::from_secs(5), &mut download).await {
                    result?;
                }
            },
            result = &mut download => { result?; },
        }
    }
    output.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> String {
        serde_json::to_string(&NativeHandshake {
            kind: "jcode-native-stdio".into(),
            protocol: PROTOCOL,
            version: "test-build".into(),
            working_dir: "/remote/home".into(),
            socket_path: "/remote/native.sock".into(),
        })
        .unwrap()
            + "\n"
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(script);
        command
    }

    #[test]
    fn rejects_option_and_shell_injection() {
        for host in [
            "",
            "-oProxyCommand=bad",
            "host;echo",
            "user@host@other",
            "$(id)",
            "user name@host",
            "host\nother",
        ] {
            assert!(SshOptions::new(host, "jcode").is_err(), "{host}");
        }
        for host in ["jcode-dev", "user@host", "[::1]", "host.example"] {
            assert!(SshOptions::new(host, "jcode").is_ok(), "{host}");
        }
        for binary in ["", "--help", "jcode\nfalse"] {
            assert!(SshOptions::new("host", binary).is_err());
        }
    }

    #[test]
    fn command_is_owned_noninteractive_and_quotes_literal_paths() {
        let mut options = SshOptions::new("user@jcode-dev", "/a path/jcode'quoted").unwrap();
        options.daemon_socket = Some("/socket path/native'quoted".into());
        options.working_dir = Some("/workspace with 'quotes'".into());
        let command = options.command();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        for required in [
            "-T",
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ForwardAgent=no",
            "ClearAllForwardings=yes",
            "ControlMaster=no",
            "none",
            "ForkAfterAuthentication=no",
        ] {
            assert!(args.iter().any(|s| s == required), "{required}");
        }
        let remote = args.last().unwrap();
        assert!(remote.contains("exec '/a path/jcode'\\''quoted'"));
        assert!(remote.contains("--socket '/socket path/native'\\''quoted'"));
        assert!(remote.contains("--cwd '/workspace with '\\''quotes'\\'''"));
        assert!(remote.ends_with("server stdio"));
        assert_eq!(args[args.len() - 2], "user@jcode-dev");
    }

    #[tokio::test]
    async fn handshake_preserves_following_native_bytes() {
        let wire = hello() + "{\"event\":\"native\"}\n";
        let mut reader = BufReader::new(wire.as_bytes());
        let header = read_handshake(&mut reader).await.unwrap();
        assert_eq!(header.working_dir, "/remote/home");
        let mut remainder = String::new();
        reader.read_to_string(&mut remainder).await.unwrap();
        assert_eq!(remainder, "{\"event\":\"native\"}\n");
    }

    #[tokio::test]
    async fn handshake_rejects_contamination_unknown_protocol_and_oversize() {
        for wire in [
            "Welcome!\n".to_owned() + &hello(),
            hello().replace("\"protocol\":1", "\"protocol\":99"),
            "x".repeat(HANDSHAKE_LIMIT + 1),
            String::new(),
        ] {
            let mut reader = BufReader::new(wire.as_bytes());
            assert!(read_handshake(&mut reader).await.is_err());
        }
    }

    #[tokio::test]
    async fn startup_timeout_kills_and_reaps_owned_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let script = format!("echo $$ > '{}'; exec sleep 60", pid_file.display());
        let start = std::time::Instant::now();
        let result = SshConnection::spawn(shell(&script), Duration::from_millis(100)).await;
        let error = result.err().expect("must time out");
        assert!(format!("{error:#}").contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3));
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "child must be reaped");
    }

    #[tokio::test]
    async fn successful_connection_shutdown_reaps_child() {
        let script = format!("printf '{}'; exec sleep 60", hello());
        let mut connection = SshConnection::spawn(shell(&script), Duration::from_secs(2))
            .await
            .unwrap();
        let pid = connection.child.0.id().unwrap() as i32;
        assert_eq!(connection.handshake.protocol, PROTOCOL);
        connection.shutdown().await;
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[tokio::test]
    async fn cancelled_handshake_retains_child_for_explicit_reaping() {
        let mut connection = SshConnection::spawn_process(shell("exec sleep 60")).unwrap();
        let pid = connection.child.0.id().unwrap() as i32;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                connection.establish(STARTUP_TIMEOUT)
            )
            .await
            .is_err()
        );
        connection.shutdown().await;
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[tokio::test]
    async fn guard_drop_removes_private_socket_and_signals_children() {
        use std::os::unix::fs::PermissionsExt;
        let directory = private_directory().unwrap();
        let root = directory.path().to_path_buf();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let socket = root.join("native.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (stop, mut stopped) = watch::channel(false);
        let guard = NativeSsh {
            _directory: directory,
            socket: socket.clone(),
            host: "test".into(),
            handshake: serde_json::from_str(&hello()).unwrap(),
            stop,
            manager: None,
        };
        assert!(socket.exists());
        drop(guard);
        assert!(!socket.exists());
        assert!(!root.exists());
        stopped.changed().await.unwrap();
        assert!(*stopped.borrow());
        drop(listener);
    }

    #[tokio::test]
    async fn explicit_close_waits_for_child_reaping_and_is_idempotent() {
        let mut child = SshConnection::spawn(
            shell(&format!("printf '{}'; exec sleep 60", hello())),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let pid = child.child.0.id().unwrap() as i32;
        let directory = private_directory().unwrap();
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (stop, mut stopped) = watch::channel(false);
        let manager = tokio::spawn(async move {
            let _ = stopped.changed().await;
            child.shutdown().await;
            drop(listener);
        });
        let mut guard = NativeSsh {
            _directory: directory,
            socket: socket.clone(),
            host: "test".into(),
            handshake: serde_json::from_str(&hello()).unwrap(),
            stop,
            manager: Some(manager),
        };
        guard.close().await.unwrap();
        assert!(!socket.exists());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        guard.close().await.unwrap();
    }

    #[tokio::test]
    async fn workspace_validation_rejects_empty_and_control_paths_before_ssh() {
        for path in ["", "bad\npath", "bad\0path"] {
            let error = NativeSsh::connect_with_workspace("test", "jcode", None, Some(path))
                .await
                .err()
                .expect("invalid path must fail");
            assert!(error.to_string().contains("remote working directory"));
        }
    }

    #[tokio::test]
    async fn failed_startup_reports_bounded_stderr() {
        let error = SshConnection::spawn(
            shell("printf 'Host key verification failed' >&2; exit 255"),
            Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("Host key verification failed"));
        let script = format!(
            "i=0; while [ $i -lt 5000 ]; do printf 0123456789 >&2; i=$((i+1)); done; printf '{}' ; exec sleep 60",
            hello().trim_end()
        );
        // No newline ensures timeout after stderr has been fully drained.
        let error = SshConnection::spawn(shell(&script), Duration::from_millis(500))
            .await
            .err()
            .unwrap();
        assert!(format!("{error:#}").len() < STDERR_LIMIT + 1024);
    }

    #[tokio::test]
    async fn bridge_exchanges_native_frames_and_exits_when_daemon_dies() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (client, bridge) = tokio::io::duplex(4096);
        let (input, output) = tokio::io::split(bridge);
        let task = tokio::spawn(bridge_stream(input, output, socket));
        let (daemon, _) = listener.accept().await.unwrap();
        let (read, mut write) = daemon.into_split();
        let mut read = BufReader::new(read);
        let mut line = String::new();
        read.read_line(&mut line).await.unwrap();
        assert_eq!(line, "{\"type\":\"ping\",\"id\":0}\n");
        write
            .write_all(b"{\"type\":\"pong\",\"id\":0,\"native_ssh_protocol\":1}\n")
            .await
            .unwrap();
        let (mut client_read, mut client_write) = tokio::io::split(client);
        let mut client_read = BufReader::new(&mut client_read);
        read_handshake(&mut client_read).await.unwrap();
        client_write
            .write_all(b"{\"type\":\"ping\",\"id\":1}\n")
            .await
            .unwrap();
        line.clear();
        read.read_line(&mut line).await.unwrap();
        assert!(line.contains("ping"));
        write
            .write_all(b"{\"type\":\"pong\",\"id\":1}\n")
            .await
            .unwrap();
        line.clear();
        client_read.read_line(&mut line).await.unwrap();
        assert!(line.contains("pong"));
        drop(read);
        drop(write);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        // client_write deliberately remains open: daemon EOF must still exit.
        drop(client_write);
    }

    #[tokio::test]
    async fn bridge_does_not_claim_handshake_without_daemon() {
        let (client, bridge) = tokio::io::duplex(1024);
        let (input, output) = tokio::io::split(bridge);
        let directory = tempfile::tempdir().unwrap();
        assert!(
            bridge_stream(input, output, directory.path().join("missing.sock"))
                .await
                .is_err()
        );
        let mut client = client;
        let mut output = Vec::new();
        client.read_to_end(&mut output).await.unwrap();
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn stdio_eof_half_closes_daemon_and_drains_last_reply() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (client, bridge) = tokio::io::duplex(4096);
        let (input, output) = tokio::io::split(bridge);
        let task = tokio::spawn(bridge_stream(input, output, socket));
        let (daemon, _) = listener.accept().await.unwrap();
        let (read, mut write) = daemon.into_split();
        let mut read = BufReader::new(read);
        let mut line = String::new();
        read.read_line(&mut line).await.unwrap();
        write
            .write_all(b"{\"type\":\"pong\",\"id\":0,\"native_ssh_protocol\":1}\n")
            .await
            .unwrap();
        let (read_client, mut write_client) = tokio::io::split(client);
        let mut read_client = BufReader::new(read_client);
        read_handshake(&mut read_client).await.unwrap();
        write_client
            .write_all(b"{\"type\":\"ping\",\"id\":1}\n")
            .await
            .unwrap();
        write_client.shutdown().await.unwrap();
        line.clear();
        read.read_line(&mut line).await.unwrap();
        assert!(line.contains("ping"));
        line.clear();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), read.read_line(&mut line))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        // Deliberately reply only after observing the client's write EOF.
        write
            .write_all(b"{\"type\":\"pong\",\"id\":1}\n")
            .await
            .unwrap();
        drop(write);
        drop(read);
        read_client.read_to_string(&mut line).await.unwrap();
        assert!(line.contains("pong"));
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn daemon_capability_requires_supported_protocol_and_matching_ping() {
        for frame in [
            "{\"type\":\"pong\",\"id\":0}\n",
            "{\"type\":\"pong\",\"id\":0,\"native_ssh_protocol\":2}\n",
            "{\"type\":\"pong\",\"id\":1,\"native_ssh_protocol\":1}\n",
        ] {
            let mut reader = BufReader::new(frame.as_bytes());
            let mut sink = tokio::io::sink();
            let error = verify_daemon_protocol(&mut reader, &mut sink)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("update/reload"));
        }
        let frame = b"{\"type\":\"pong\",\"id\":0,\"native_ssh_protocol\":1}\nnative-frame\n";
        let mut reader = BufReader::new(&frame[..]);
        verify_daemon_protocol(&mut reader, &mut tokio::io::sink())
            .await
            .unwrap();
        let mut remaining = String::new();
        reader.read_to_string(&mut remaining).await.unwrap();
        assert_eq!(remaining, "native-frame\n");
    }
}
