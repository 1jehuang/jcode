//! Best-effort, bounded local upgrade negotiation. Never falls back to Reload.
use super::Progress;
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_FRAME: usize = 64 * 1024;

pub(super) fn is_local_shared(api: &Path, native: &Path, candidate: &Path) -> bool {
    let runtime = jcode_harness_api::runtime_dir();
    candidate.is_absolute()
        && api == runtime.join("jcode-api.sock")
        && native == runtime.join("jcode.sock")
}

fn connect(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, timeout)?;
    Ok(socket.into())
}

fn read_frame(stream: &mut UnixStream, deadline: Instant) -> io::Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        let mut byte = [0];
        if stream.read(&mut byte)? == 0 {
            return Ok(None);
        }
        if byte[0] == b'\n' {
            return serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(io::Error::other);
        }
        bytes.push(byte[0]);
        if bytes.len() > MAX_FRAME {
            return Err(io::Error::other("runtime response too large"));
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Unchanged,
    Busy(String),
    Deferred(String),
    ReconnectPending,
}

fn request(socket: &Path, candidate: &Path, timeout: Duration) -> io::Result<Outcome> {
    let deadline = Instant::now() + timeout;
    let mut stream = connect(socket, timeout)?;
    stream.set_write_timeout(Some(timeout))?;
    let mut frame =
        serde_json::to_vec(&json!({"type":"upgrade_runtime", "id":1, "candidate":candidate}))
            .map_err(io::Error::other)?;
    frame.push(b'\n');
    stream.write_all(&frame)?;
    let mut upgrading = false;
    loop {
        let Some(reply) = read_frame(&mut stream, deadline)? else {
            return if upgrading {
                Ok(Outcome::ReconnectPending)
            } else {
                Err(io::Error::other(
                    "runtime disconnected before upgrade negotiation",
                ))
            };
        };
        match reply.get("type").and_then(Value::as_str) {
            Some("ack") => continue,
            Some("reload_progress") => match reply.get("step").and_then(Value::as_str) {
                Some("unchanged") => return Ok(Outcome::Unchanged),
                Some("busy") => return Ok(Outcome::Busy(reply.get("message").and_then(Value::as_str).unwrap_or("Runtime is busy").into())),
                Some("upgrading") => { upgrading = true; continue; }
                _ => return Ok(Outcome::Deferred(reply.get("message").and_then(Value::as_str).unwrap_or("Runtime upgrade deferred; existing work was not interrupted.").into())),
            },
            // Old daemons reject the new request. Never retry with the old
            // force/default reload wire format, which interrupts active work.
            Some("error") => return Ok(Outcome::Deferred("The running runtime predates safe bundled upgrades. Continuing without restarting it. Restart the runtime once all work is idle to use the bundled version.".into())),
            _ => return Err(io::Error::other("unrecognized runtime upgrade response")),
        }
    }
}

pub(super) fn offer(
    socket: &Path,
    candidate: &Path,
    startup_timeout: Duration,
    progress: &Progress<'_>,
) {
    match request(
        socket,
        candidate,
        NEGOTIATION_TIMEOUT
            .min(startup_timeout)
            .max(Duration::from_millis(1)),
    ) {
        Ok(Outcome::Unchanged) => {}
        Ok(Outcome::Busy(message)) => {
            progress(&format!(
                "{message} Pending bundled upgrade will retry safely every 30 seconds for up to 10 minutes. If work remains busy, reopen the app once idle to retry."
            ));
            schedule_retry(socket, candidate);
        }
        Ok(Outcome::Deferred(message)) => progress(&message),
        Ok(Outcome::ReconnectPending) => {
            progress("Bundled runtime switch requested while idle. Verifying runtime reconnect...");
            match confirm_reconnect(socket, candidate, startup_timeout) {
                Ok(()) => progress("Bundled runtime reconnect verified; runtime is up to date."),
                Err(error) => progress(&format!(
                    "Runtime upgrade reconnect not yet verified ({error}). No forced restart was attempted."
                )),
            }
        }
        Err(error) => progress(&format!(
            "Safe runtime upgrade check deferred ({error}). Continuing without forcing a restart."
        )),
    }
}

fn confirm_reconnect(socket: &Path, candidate: &Path, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match request(socket, candidate, left.min(NEGOTIATION_TIMEOUT)) {
            // The new daemon independently re-probes the candidate and confirms
            // its running update version is at least as new. Connect/EOF alone
            // cannot establish that exec succeeded or the runtime is ready.
            Ok(Outcome::Unchanged) => return Ok(()),
            Ok(Outcome::Deferred(message)) => return Err(io::Error::other(message)),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(io::ErrorKind::TimedOut.into())
}

fn retry_until_idle(
    socket: &Path,
    candidate: &Path,
    attempts: usize,
    delay: Duration,
) -> io::Result<Outcome> {
    for _ in 0..attempts {
        std::thread::sleep(delay);
        match request(socket, candidate, NEGOTIATION_TIMEOUT)? {
            Outcome::Busy(_) => continue,
            outcome => return Ok(outcome),
        }
    }
    Ok(Outcome::Deferred("Bundled upgrade remains pending because the runtime stayed busy. Once work is idle, reopen the app to retry the safe bundled upgrade.".into()))
}

fn schedule_retry(socket: &Path, candidate: &Path) {
    use std::collections::HashSet;
    use std::sync::{LazyLock, Mutex};
    type Key = (std::path::PathBuf, std::path::PathBuf);
    static PENDING: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    let key = (socket.to_path_buf(), candidate.to_path_buf());
    let Ok(mut pending) = PENDING.lock() else {
        return;
    };
    if !pending.insert(key.clone()) {
        return;
    }
    drop(pending);
    let cleanup_key = key.clone();
    if std::thread::Builder::new()
        .name("jcode-idle-upgrade".into())
        .spawn(move || {
            match retry_until_idle(&key.0, &key.1, 20, Duration::from_secs(30)) {
                Ok(Outcome::ReconnectPending) => {
                    if let Err(error) = confirm_reconnect(&key.0, &key.1, Duration::from_secs(30)) {
                        eprintln!("jcode runtime: pending upgrade reconnect unverified: {error}");
                    }
                }
                Ok(Outcome::Deferred(message)) => eprintln!("jcode runtime: {message}"),
                Err(error) => eprintln!(
                    "jcode runtime: pending upgrade check stopped without restart: {error}"
                ),
                _ => {}
            }
            if let Ok(mut pending) = PENDING.lock() {
                pending.remove(&key);
            }
        })
        .is_err()
    {
        if let Ok(mut pending) = PENDING.lock() {
            pending.remove(&cleanup_key);
        }
    }
}

#[cfg(test)]
#[path = "launch_upgrade_tests.rs"]
mod tests;
