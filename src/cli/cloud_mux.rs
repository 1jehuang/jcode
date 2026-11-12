//! One shared, authenticated SSH connection to the managed cloud machine.
//!
//! A new SSH connection costs a control-plane authorization plus a full key
//! exchange through `jcode cloud proxy`: several seconds, and far longer when
//! many clients do it at once, because the API slows down and sshd starts
//! dropping unauthenticated connections (`MaxStartups`). A session opened on
//! an existing master takes a few hundred milliseconds and touches neither.
//!
//! Every managed caller already passes `ControlMaster=no`, so it only borrows
//! a master. This module is what creates one: a detached `ssh -N -f` that
//! outlives the process that started it and exits `ControlPersist` seconds
//! after its last session closes, so an idle machine can still go to sleep.

use anyhow::{Context, Result, bail};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::cloud_managed;

/// Matches the owned transport's dead-peer detection (2 x 15s). Sessions on a
/// master inherit its keepalive, not their own.
const KEEPALIVE: [&str; 4] = [
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
];

fn base(config: &[String]) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(config)
        .args(["-o", "BatchMode=yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

fn master_running(config: &[String]) -> bool {
    // Also unlinks a stale socket left by a master that was killed.
    base(config)
        .args(["-O", "check", "--", cloud_managed::HOST])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn master_args() -> Vec<&'static str> {
    let mut args = vec![
        "-N",
        "-f",
        "-T",
        "-C",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "ControlMaster=auto",
        "-o",
        "ConnectTimeout=30",
    ];
    args.extend(KEEPALIVE);
    args
}

/// Make sure a shared master exists. Returns false when multiplexing is not
/// available (signed out), in which case callers connect directly as before.
pub(crate) fn ensure_master(deadline: Duration) -> Result<bool> {
    // Signed-out configs disable multiplexing. A detached `ssh -N` without a
    // control socket would be an unreachable connection that never exits.
    if cloud_managed::preparation_namespace().is_err() {
        return Ok(false);
    }
    let config = cloud_managed::ssh_args()?;
    if master_running(&config) {
        return Ok(true);
    }
    let started = Instant::now();
    let dir = cloud_managed::dir()?;
    // Serialize creation across every local client. A loser of an unlocked
    // race cannot bind the socket and would linger as a plain connection.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("master.lock"))
        .context("open cloud master lock")?;
    lock.lock().context("lock cloud master creation")?;
    if master_running(&config) {
        return Ok(true);
    }
    let result = spawn_master(&config, &dir, deadline.saturating_sub(started.elapsed()));
    crate::logging::info(&format!(
        "[CLOUD_TIMING] stage=ssh_master outcome={} duration_ms={:.3}",
        if result.is_ok() { "ok" } else { "error" },
        started.elapsed().as_secs_f64() * 1000.0
    ));
    result.map(|()| true)
}

fn spawn_master(config: &[String], dir: &std::path::Path, deadline: Duration) -> Result<()> {
    let log_path = dir.join("master.log");
    // The master and its proxy outlive this process, so they must not hold
    // the caller's terminal or a pipe it will close.
    let log = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&log_path)
            .context("open cloud master log")?
    };
    let mut child = base(config)
        .args(master_args())
        .args(["--", cloud_managed::HOST])
        .stderr(Stdio::from(log))
        // Its own group: a terminal hangup or the owned transport's group
        // kill must not take the shared connection down with one client.
        .process_group(0)
        .spawn()
        .context("start the shared cloud SSH connection")?;
    // `-f` returns once authentication succeeded and the master detached.
    let until = Instant::now() + deadline;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= until {
            // SAFETY: signals only the process group created above.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            bail!("shared cloud SSH connection timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() || !master_running(config) {
        let detail = std::fs::read_to_string(&log_path).unwrap_or_default();
        let detail = detail.lines().last().unwrap_or_default();
        bail!("shared cloud SSH connection failed ({status}): {detail}");
    }
    Ok(())
}

/// Best-effort async wrapper for transports. A failure only costs the
/// speedup: the caller's own `ssh` still connects directly.
pub(crate) async fn ensure(host: &str, deadline: Duration) {
    if !cloud_managed::is_managed(host) {
        return;
    }
    // A plain thread, not `spawn_blocking`: runtime shutdown waits for
    // blocking tasks, and this may sit in a network timeout.
    let (done, result) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("cloud-ssh-master".into())
        .spawn(move || {
            let _ = done.send(ensure_master(deadline));
        });
    if spawned.is_err() {
        return;
    }
    // The lock wait and the connect are each bounded by `deadline`.
    let outcome = tokio::time::timeout(deadline * 2 + Duration::from_secs(5), result).await;
    if let Ok(Ok(Err(error))) = outcome {
        crate::logging::warn(&format!(
            "Shared cloud SSH connection unavailable, connecting directly: {error:#}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_detaches_owns_the_socket_and_cannot_forward() {
        let args = master_args();
        for required in [
            "-N",
            "-f",
            "-C",
            "ControlMaster=auto",
            "ForwardAgent=no",
            "ClearAllForwardings=yes",
            "ServerAliveInterval=15",
        ] {
            assert!(args.contains(&required), "{required}");
        }
        // Lifetime comes from the managed config, so an idle machine sleeps.
        assert!(!args.iter().any(|arg| arg.starts_with("ControlPersist")));
        assert!(!args.iter().any(|arg| arg.starts_with("ControlPath")));
    }

    #[tokio::test]
    async fn other_hosts_never_get_a_shared_master() {
        // Returns without touching the account, the network, or `ssh`.
        tokio::time::timeout(
            Duration::from_secs(1),
            ensure("user@example.invalid", Duration::from_secs(60)),
        )
        .await
        .unwrap();
    }
}
