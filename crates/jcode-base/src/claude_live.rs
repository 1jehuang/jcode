//! Discovery and explicit handoff support for live Claude Code sessions.
//!
//! Claude Code 2.1.x publishes one small registry record per interactive
//! process at `~/.claude/sessions/<pid>.json`. The record's `procStart` value
//! matches Linux `/proc/<pid>/stat` field 22, which lets us guard against PID
//! reuse before presenting or signaling a process.

#[cfg(not(target_os = "linux"))]
use anyhow::anyhow;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionRegistryRecord {
    pid: u32,
    session_id: String,
    cwd: String,
    proc_start: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    started_at: Option<i64>,
    #[serde(default)]
    version: Option<String>,
}

/// A Claude Code process whose registry record and OS process identity agree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveClaudeSession {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub proc_start: String,
    pub name: Option<String>,
    pub started_at: Option<i64>,
    pub version: Option<String>,
    registry_path: PathBuf,
}

/// Result of asking the identity-verified Claude process to exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopLiveClaudeOutcome {
    /// The stable process handle reported that Claude exited.
    Exited,
    /// SIGTERM was delivered, but exit was not observed before the deadline.
    ExitUnconfirmed,
}

impl LiveClaudeSession {
    fn from_record(record: ClaudeSessionRegistryRecord, registry_path: PathBuf) -> Self {
        Self {
            pid: record.pid,
            session_id: record.session_id,
            cwd: record.cwd,
            proc_start: record.proc_start,
            name: record.name,
            started_at: record.started_at,
            version: record.version,
            registry_path,
        }
    }
}

/// Return live, identity-verified interactive Claude Code CLI sessions.
///
/// On platforms where Claude's process-start token cannot yet be verified, the
/// safe behavior is to return no takeover candidates rather than trust a PID.
pub fn live_claude_sessions() -> Result<Vec<LiveClaudeSession>> {
    let root = crate::storage::user_home_path(".claude/sessions")?;
    live_claude_sessions_in(&root)
}

pub fn find_live_claude_session(session_id: &str) -> Result<Option<LiveClaudeSession>> {
    Ok(live_claude_sessions()?
        .into_iter()
        .find(|session| session.session_id == session_id))
}

fn live_claude_sessions_in(root: &Path) -> Result<Vec<LiveClaudeSession>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(root).with_context(|| {
        format!(
            "failed to read Claude live-session registry {}",
            root.display()
        )
    })? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<ClaudeSessionRegistryRecord>(&bytes) else {
            continue;
        };
        if !registry_record_is_takeover_candidate(&path, &record) {
            continue;
        }
        if process_identity_matches(record.pid, &record.proc_start) {
            sessions.push(LiveClaudeSession::from_record(record, path));
        }
    }
    sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(sessions)
}

fn registry_record_is_takeover_candidate(
    path: &Path,
    record: &ClaudeSessionRegistryRecord,
) -> bool {
    if record.session_id.trim().is_empty()
        || record.cwd.trim().is_empty()
        || record.proc_start.trim().is_empty()
    {
        return false;
    }
    if record
        .kind
        .as_deref()
        .is_some_and(|kind| kind != "interactive")
        || record
            .entrypoint
            .as_deref()
            .is_some_and(|entrypoint| entrypoint != "cli")
    {
        return false;
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse::<u32>().ok())
        == Some(record.pid)
}

#[cfg(target_os = "linux")]
fn linux_process_identity(pid: u32) -> std::io::Result<(String, char)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close_paren = stat
        .rfind(')')
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid proc stat"))?;
    let fields = stat
        .get(close_paren + 2..)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid proc stat"))?
        .split_whitespace()
        .collect::<Vec<_>>();
    let state = fields
        .first()
        .and_then(|field| field.chars().next())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing state"))?;
    // `/proc/<pid>/stat` field 22 is the process start time in clock ticks.
    // `fields[0]` here is overall field 3 (`state`), hence index 19.
    let start = fields
        .get(19)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing start"))?;
    Ok(((*start).to_string(), state))
}

#[cfg(target_os = "linux")]
fn process_identity_matches(pid: u32, expected_start: &str) -> bool {
    linux_process_identity(pid)
        .map(|(actual_start, state)| state != 'Z' && actual_start == expected_start)
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn process_identity_matches(_pid: u32, _expected_start: &str) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn open_pidfd(pid: u32) -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) })
}

#[cfg(target_os = "linux")]
fn send_sigterm_via_pidfd(pidfd: &OwnedFd) -> std::io::Result<bool> {
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            libc::SIGTERM,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if matches!(err.raw_os_error(), Some(code) if code == libc::ESRCH) {
        return Ok(false);
    }
    Err(err)
}

#[cfg(target_os = "linux")]
fn wait_for_pidfd_exit(pidfd: &OwnedFd, timeout: Duration) -> std::io::Result<bool> {
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let mut pollfd = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let rc = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if rc > 0 {
            return Ok(true);
        }
        if rc == 0 {
            return Ok(false);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn registry_still_matches(session: &LiveClaudeSession) -> bool {
    let Ok(bytes) = std::fs::read(&session.registry_path) else {
        return false;
    };
    serde_json::from_slice::<ClaudeSessionRegistryRecord>(&bytes).is_ok_and(|record| {
        record.pid == session.pid
            && record.session_id == session.session_id
            && record.proc_start == session.proc_start
    })
}

fn remove_registry_if_same(session: &LiveClaudeSession) {
    if registry_still_matches(session) {
        let _ = std::fs::remove_file(&session.registry_path);
    }
}

/// Gracefully stop the exact Claude Code process represented by `session`.
///
/// The process-start token is checked again immediately before signaling. A
/// PID that has exited or been reused is never signaled. This function sends a
/// single SIGTERM and does not escalate to SIGKILL.
pub fn stop_live_claude_session(
    session: &LiveClaudeSession,
    timeout: Duration,
) -> Result<StopLiveClaudeOutcome> {
    if !registry_still_matches(session) {
        bail!(
            "Claude Code session {} changed or closed before takeover",
            session.session_id
        );
    }

    #[cfg(target_os = "linux")]
    let pidfd = open_pidfd(session.pid).with_context(|| {
        format!(
            "failed to open a stable handle for Claude Code process {}",
            session.pid
        )
    })?;

    if !process_identity_matches(session.pid, &session.proc_start) {
        bail!(
            "Claude Code session {} is no longer owned by the recorded process",
            session.session_id
        );
    }
    if !registry_still_matches(session) {
        bail!(
            "Claude Code session {} changed or closed before takeover",
            session.session_id
        );
    }

    #[cfg(target_os = "linux")]
    {
        let signal_sent = send_sigterm_via_pidfd(&pidfd)
            .with_context(|| format!("failed to stop Claude Code process {}", session.pid))?;
        if !signal_sent {
            remove_registry_if_same(session);
            return Ok(StopLiveClaudeOutcome::Exited);
        }
        if wait_for_pidfd_exit(&pidfd, timeout).with_context(|| {
            format!(
                "failed while waiting for Claude Code process {}",
                session.pid
            )
        })? {
            remove_registry_if_same(session);
            return Ok(StopLiveClaudeOutcome::Exited);
        }
        return Ok(StopLiveClaudeOutcome::ExitUnconfirmed);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = timeout;
        return Err(anyhow!(
            "live Claude Code takeover is not yet supported on this platform"
        ));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    fn spawn_sleep() -> Child {
        Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn write_record(root: &Path, child: &Child, session_id: &str, start: &str) -> PathBuf {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join(format!("{}.json", child.id()));
        std::fs::write(
            &path,
            serde_json::json!({
                "pid": child.id(),
                "sessionId": session_id,
                "cwd": "/tmp/project",
                "procStart": start,
                "kind": "interactive",
                "entrypoint": "cli",
                "startedAt": 123,
                "name": "probe",
                "version": "2.1.212"
            })
            .to_string(),
        )
        .unwrap();
        path
    }

    #[test]
    fn discovery_requires_matching_process_start_token() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut child = spawn_sleep();
        let (start, _) = linux_process_identity(child.id()).unwrap();
        write_record(temp.path(), &child, "live", &start);

        let live = live_claude_sessions_in(temp.path()).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].session_id, "live");

        write_record(temp.path(), &child, "stale", "not-the-start-token");
        assert!(live_claude_sessions_in(temp.path()).unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn stop_signals_only_the_identity_verified_process() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut target = spawn_sleep();
        let mut unrelated = spawn_sleep();
        let (start, _) = linux_process_identity(target.id()).unwrap();
        let path = write_record(temp.path(), &target, "takeover", &start);
        let session = live_claude_sessions_in(temp.path()).unwrap().remove(0);

        assert_eq!(
            stop_live_claude_session(&session, Duration::from_secs(2)).unwrap(),
            StopLiveClaudeOutcome::Exited
        );
        target.wait().unwrap();
        assert!(unrelated.try_wait().unwrap().is_none());
        assert!(!path.exists());

        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[test]
    fn pidfd_never_retargets_after_the_original_process_exits() {
        let mut target = spawn_sleep();
        let pidfd = open_pidfd(target.id()).unwrap();
        target.kill().unwrap();
        target.wait().unwrap();

        let mut unrelated = spawn_sleep();
        assert!(!send_sigterm_via_pidfd(&pidfd).unwrap());
        assert!(unrelated.try_wait().unwrap().is_none());

        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[test]
    fn mismatched_identity_is_never_signaled() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut child = spawn_sleep();
        let path = write_record(temp.path(), &child, "mismatch", "wrong");
        let session = LiveClaudeSession {
            pid: child.id(),
            session_id: "mismatch".to_string(),
            cwd: "/tmp/project".to_string(),
            proc_start: "wrong".to_string(),
            name: None,
            started_at: None,
            version: None,
            registry_path: path,
        };

        assert!(stop_live_claude_session(&session, Duration::from_millis(50)).is_err());
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn changed_registry_session_is_never_signaled() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut child = spawn_sleep();
        let (start, _) = linux_process_identity(child.id()).unwrap();
        let path = write_record(temp.path(), &child, "original", &start);
        let session = live_claude_sessions_in(temp.path()).unwrap().remove(0);
        std::fs::write(
            &path,
            serde_json::json!({
                "pid": child.id(),
                "sessionId": "different-session",
                "cwd": "/tmp/project",
                "procStart": start,
                "kind": "interactive",
                "entrypoint": "cli"
            })
            .to_string(),
        )
        .unwrap();

        assert!(stop_live_claude_session(&session, Duration::from_millis(50)).is_err());
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
