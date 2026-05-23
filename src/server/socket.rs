use super::Client;
use crate::transport::Stream;
use anyhow::Result;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_SOCKET") {
        return PathBuf::from(custom);
    }
    crate::storage::runtime_dir().join("jcode.sock")
}

/// Debug socket path for testing/introspection
/// Derived from main socket path
pub fn debug_socket_path() -> PathBuf {
    let main_path = socket_path();
    let filename = main_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("jcode.sock");
    let debug_filename = filename.replace(".sock", "-debug.sock");
    main_path.with_file_name(debug_filename)
}

pub(super) fn sibling_socket_path(path: &std::path::Path) -> Option<PathBuf> {
    let filename = path.file_name()?.to_str()?;

    if let Some(base) = filename.strip_suffix("-debug.sock") {
        return Some(path.with_file_name(format!("{}.sock", base)));
    }

    if let Some(base) = filename.strip_suffix(".sock") {
        return Some(path.with_file_name(format!("{}-debug.sock", base)));
    }

    None
}

/// Remove a socket file and its sibling (main/debug) if present.
pub fn cleanup_socket_pair(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    if let Some(sibling) = sibling_socket_path(path) {
        let _ = std::fs::remove_file(sibling);
    }
}

/// Connect to a socket path.
///
/// Do not unlink the path on connection-refused here. A client-side cleanup can
/// strand a live daemon behind an unlinked Unix socket pathname, leaving the
/// process running with the daemon lock held while new clients can no longer
/// discover or connect to it.
pub async fn connect_socket(path: &std::path::Path) -> Result<Stream> {
    match Stream::connect(path).await {
        Ok(stream) => Ok(stream),
        Err(err) if err.kind() == std::io::ErrorKind::ConnectionRefused && path.exists() => {
            anyhow::bail!(
                "Socket exists but refused the connection at {}. Retry, or remove it after confirming no jcode server is running.",
                path.display()
            )
        }
        Err(err) if err.raw_os_error() == Some(libc::EMFILE) => Err(anyhow::anyhow!(
            "{} ({})",
            err,
            crate::util::process_fd_diagnostic_snapshot()
        )),
        Err(err) => Err(err.into()),
    }
}

pub(super) async fn socket_has_live_listener(path: &std::path::Path) -> bool {
    crate::transport::is_socket_path(path) && Stream::connect(path).await.is_ok()
}

/// Return true if a live server process is listening on the socket path.
///
/// This is intentionally weaker than [`is_server_ready`]: a live listener may
/// still be finishing startup or be temporarily too busy to answer a ping
/// within the short readiness timeout. Callers that must avoid spawning a
/// duplicate daemon should prefer this check over a ping-only probe.
pub async fn has_live_listener(path: &std::path::Path) -> bool {
    socket_has_live_listener(path).await
}

#[cfg(unix)]
pub(super) fn daemon_lock_path() -> PathBuf {
    crate::storage::runtime_dir().join("jcode-daemon.lock")
}

#[cfg(unix)]
pub(super) struct DaemonLockGuard {
    _file: std::fs::File,
    path: PathBuf,
}

#[cfg(unix)]
impl Drop for DaemonLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
pub(super) fn try_acquire_daemon_lock(path: &std::path::Path) -> Result<Option<DaemonLockGuard>> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let fd = file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        Ok(Some(DaemonLockGuard {
            _file: file,
            path: path.to_path_buf(),
        }))
    } else {
        Ok(None)
    }
}

#[cfg(unix)]
pub(super) fn acquire_daemon_lock() -> Result<DaemonLockGuard> {
    let path = daemon_lock_path();
    try_acquire_daemon_lock(&path)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Another jcode server process is already running for runtime dir {}",
            crate::storage::runtime_dir().display()
        )
    })
}

#[cfg(unix)]
pub(super) fn mark_close_on_exec<T: std::os::fd::AsRawFd>(io: &T) {
    let fd = io.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags >= 0 {
        let _ = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    }
}

pub fn set_socket_path(path: &str) {
    crate::env::set_var("JCODE_SOCKET", path);
}

/// Spawn a server child process and wait until it signals readiness.
///
/// Creates an anonymous pipe, passes the write-end fd to the child via
/// `JCODE_READY_FD`, and awaits a single byte on the read end. The server
/// calls `signal_ready_fd()` once its accept loops are spawned, so the future
/// resolves only after the daemon can start servicing client requests.
///
/// Falls back to a short poll loop if the pipe read times out (e.g. server
/// built without ready-fd support, or crash before bind).
#[cfg(unix)]
pub async fn spawn_server_notify(cmd: &mut std::process::Command) -> Result<std::process::Child> {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;

    // Create a pipe: fds[0] = read end, fds[1] = write end.
    // Use pipe2 with O_CLOEXEC on the read end (parent keeps it).
    // The write end needs CLOEXEC cleared so it survives exec in the child.
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        anyhow::bail!("pipe() failed: {}", std::io::Error::last_os_error());
    }
    let read_fd = fds[0];
    let write_fd = fds[1];

    // Set CLOEXEC on the read end (parent only)
    unsafe {
        let flags = libc::fcntl(read_fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(read_fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }

    // Pass the write-end fd to the child and tell it the fd number.
    unsafe {
        cmd.pre_exec(move || {
            // Clear CLOEXEC on the write end so it survives exec
            let flags = libc::fcntl(write_fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(write_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
            libc::setsid();
            Ok(())
        });
    }
    cmd.env("JCODE_READY_FD", write_fd.to_string());

    let mut child = cmd.spawn()?;

    // Close our copy of the write end so we get EOF if the child dies.
    unsafe { libc::close(write_fd) };

    // Wait for the ready signal (or timeout / child death).
    let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
    let mut async_file = tokio::fs::File::from_std(read_file);
    let mut buf = [0u8; 1];
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::AsyncReadExt::read(&mut async_file, &mut buf),
    )
    .await
    {
        Ok(Ok(1)) => {
            crate::logging::info("Server signalled ready via pipe");
        }
        Ok(Ok(_)) => {
            if let Some(status) = child.try_wait()? {
                handle_server_start_exit(&mut child, status).await?;
            }
            crate::logging::info(
                "Server closed ready pipe without signalling; falling back to poll",
            );
            wait_for_server_ready(&socket_path(), Duration::from_secs(5)).await?;
        }
        Ok(Err(e)) => {
            crate::logging::info(&format!(
                "Ready pipe read error: {}; falling back to poll",
                e
            ));
            wait_for_server_ready(&socket_path(), Duration::from_secs(5)).await?;
        }
        Err(_) => {
            crate::logging::info("Timed out waiting for server ready signal; falling back to poll");
            wait_for_server_ready(&socket_path(), Duration::from_secs(5)).await?;
        }
    }

    if let Some(mut stderr) = child.stderr.take() {
        // The shared daemon outlives the spawning client. Keep draining the
        // stderr pipe after startup so later reloads cannot die on SIGPIPE
        // when they emit provider/model selection notices during boot.
        std::thread::spawn(move || {
            let mut sink = std::io::sink();
            let _ = std::io::copy(&mut stderr, &mut sink);
        });
    }

    Ok(child)
}

/// Wait until a server socket is connectable and responds to a ping.
pub async fn wait_for_server_ready(path: &std::path::Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if crate::transport::is_socket_path(path)
            && let Ok(mut client) = Client::connect_with_path(path.to_path_buf()).await
            && let Ok(Ok(true)) =
                tokio::time::timeout(Duration::from_millis(250), client.ping()).await
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!(
        "Timed out waiting for responsive server socket {}",
        path.display()
    );
}

async fn probe_server_ready(path: &std::path::Path, ping_timeout: Duration) -> bool {
    if !crate::transport::is_socket_path(path) {
        return false;
    }

    let Ok(mut client) = Client::connect_with_path(path.to_path_buf()).await else {
        return false;
    };

    matches!(
        tokio::time::timeout(ping_timeout, client.ping()).await,
        Ok(Ok(true))
    )
}

pub async fn is_server_ready(path: &std::path::Path) -> bool {
    probe_server_ready(path, Duration::from_millis(50)).await
}

#[cfg(unix)]
pub(super) fn take_server_start_stderr(child: &mut std::process::Child) -> String {
    use std::io::Read;

    child
        .stderr
        .take()
        .and_then(|mut stderr| {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf).ok()?;
            Some(buf)
        })
        .unwrap_or_default()
}

#[cfg(unix)]
pub(super) fn server_start_matches_existing_server(stderr_output: &str) -> bool {
    stderr_output.contains("Another jcode server process is already running")
        || stderr_output.contains("Refusing to replace active server socket")
}

#[cfg(any(unix, test))]
pub(super) async fn wait_for_existing_server(path: &std::path::Path, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if is_server_ready(path).await || has_live_listener(path).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[cfg(unix)]
pub(super) fn format_server_start_error(
    status: std::process::ExitStatus,
    stderr_output: &str,
) -> String {
    if stderr_output.trim().is_empty() {
        format!(
            "Server exited before signalling ready ({}). Check logs at ~/.jcode/logs/",
            status
        )
    } else {
        format!(
            "Server exited before signalling ready ({}):\n{}",
            status,
            stderr_output.trim()
        )
    }
}

#[cfg(unix)]
pub(super) async fn handle_server_start_exit(
    child: &mut std::process::Child,
    status: std::process::ExitStatus,
) -> Result<()> {
    let stderr_output = take_server_start_stderr(child);
    if server_start_matches_existing_server(&stderr_output) {
        let socket_path = socket_path();
        if wait_for_existing_server(&socket_path, Duration::from_secs(5)).await {
            crate::logging::info(
                "Server spawn raced with an existing daemon; treating startup as successful",
            );
            return Ok(());
        }
    }

    anyhow::bail!(format_server_start_error(status, &stderr_output));
}

/// Write a single byte to the fd in `JCODE_READY_FD` and close it.
/// Called after startup plumbing is ready so the parent process knows the
/// server can accept and service client requests. The env var is cleared so child
/// processes (e.g. tool subprocesses) don't inherit a stale fd.
pub(super) fn signal_ready_fd() {
    #[cfg(unix)]
    {
        use std::os::unix::io::FromRawFd;

        if let Ok(fd_str) = std::env::var("JCODE_READY_FD") {
            crate::env::remove_var("JCODE_READY_FD");
            if let Ok(fd) = fd_str.parse::<i32>() {
                let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
                let _ = std::io::Write::write_all(&mut file, b"R");
                // file is dropped here which closes the fd
            }
        }
    }
}
