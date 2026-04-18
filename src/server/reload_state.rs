use super::{has_live_listener, is_server_ready};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

pub fn reload_marker_path() -> PathBuf {
    crate::storage::runtime_dir().join("jcode.reload")
}

pub fn write_reload_marker() {
    ReloadState {
        request_id: "unknown".to_string(),
        hash: "unknown".to_string(),
        phase: ReloadPhase::Starting,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();
}

pub fn clear_reload_marker() {
    let _ = std::fs::remove_file(reload_marker_path());
}

pub(super) fn clear_reload_marker_if_stale_for_pid(current_pid: u32) {
    if let Some(state) = ReloadState::load() {
        if state.phase == ReloadPhase::Starting && state.pid == current_pid {
            return;
        }
        clear_reload_marker();
    }
}

pub fn reload_marker_exists() -> bool {
    reload_marker_path().exists()
}

pub fn reload_marker_active(max_age: Duration) -> bool {
    matches!(recent_reload_state(max_age), Some(state) if state.phase == ReloadPhase::Starting)
}

pub fn recent_reload_state(max_age: Duration) -> Option<ReloadState> {
    let path = reload_marker_path();
    let state = ReloadState::load()?;
    let Ok(metadata) = std::fs::metadata(&path) else {
        return None;
    };
    let Ok(modified) = metadata.modified() else {
        let _ = std::fs::remove_file(&path);
        return None;
    };
    let Ok(elapsed) = modified.elapsed() else {
        return Some(state);
    };
    if elapsed <= max_age {
        Some(state)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

pub fn write_reload_state(
    request_id: &str,
    hash: &str,
    phase: ReloadPhase,
    detail: Option<String>,
) {
    ReloadState {
        request_id: request_id.to_string(),
        hash: hash.to_string(),
        phase,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail,
    }
    .write();
}

pub fn publish_reload_socket_ready() {
    let Some(state) = ReloadState::load() else {
        crate::logging::warn(
            "Server reached socket-ready publish point, but no reload marker was present",
        );
        return;
    };

    let current_pid = std::process::id();
    if state.phase == ReloadPhase::Starting && state.pid == current_pid {
        write_reload_state(
            &state.request_id,
            &state.hash,
            ReloadPhase::SocketReady,
            state.detail.clone(),
        );
        crate::logging::info(&format!(
            "Published reload socket-ready state for request {}",
            state.request_id
        ));
    } else if state.phase != ReloadPhase::Starting {
        crate::logging::warn(&format!(
            "Server reached socket-ready publish point, but reload marker phase was {:?} (pid={}, current_pid={})",
            state.phase, state.pid, current_pid
        ));
    } else if state.pid != current_pid {
        crate::logging::warn(&format!(
            "Server reached socket-ready publish point, but reload marker pid {} did not match current pid {}; clearing stale marker",
            state.pid, current_pid
        ));
        clear_reload_marker();
    }
}

pub fn reload_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        matches!(err.raw_os_error(), Some(libc::EPERM))
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadWaitStatus {
    Ready,
    Waiting { pid: Option<u32> },
    Failed(Option<String>),
    Idle,
}

pub async fn inspect_reload_wait_status(
    socket_path: &std::path::Path,
    max_age: Duration,
    last_known_pid: Option<u32>,
) -> ReloadWaitStatus {
    if let Some(state) = recent_reload_state(max_age) {
        let status = match state.phase {
            ReloadPhase::SocketReady => ReloadWaitStatus::Ready,
            ReloadPhase::Failed => ReloadWaitStatus::Failed(state.detail),
            ReloadPhase::Starting => {
                if reload_process_alive(state.pid) {
                    ReloadWaitStatus::Waiting {
                        pid: Some(state.pid),
                    }
                } else {
                    ReloadWaitStatus::Failed(Some(format!(
                        "reload process {} exited before becoming ready",
                        state.pid
                    )))
                }
            }
        };
        crate::logging::info(&format!(
            "inspect_reload_wait_status: socket {} marker-driven status={:?} (last_known_pid={:?}, state={})",
            socket_path.display(),
            status,
            last_known_pid,
            reload_state_summary(max_age)
        ));
        return status;
    }

    if is_server_ready(socket_path).await || has_live_listener(socket_path).await {
        if last_known_pid.is_some() {
            crate::logging::info(&format!(
                "inspect_reload_wait_status: socket {} is ready/live without active marker (last_known_pid={:?}, state={})",
                socket_path.display(),
                last_known_pid,
                reload_state_summary(max_age)
            ));
        }
        return ReloadWaitStatus::Ready;
    }

    if let Some(pid) = last_known_pid {
        if reload_process_alive(pid) {
            crate::logging::info(&format!(
                "inspect_reload_wait_status: socket {} waiting on last known pid {} without marker",
                socket_path.display(),
                pid
            ));
            return ReloadWaitStatus::Waiting { pid: Some(pid) };
        }
        crate::logging::warn(&format!(
            "inspect_reload_wait_status: socket {} last known pid {} is no longer alive and no reload marker remains",
            socket_path.display(),
            pid
        ));
    }

    if last_known_pid.is_some() {
        crate::logging::info(&format!(
            "inspect_reload_wait_status: socket {} is idle after previous reload wait state",
            socket_path.display()
        ));
    }
    ReloadWaitStatus::Idle
}

pub async fn await_reload_handoff(
    socket_path: &std::path::Path,
    max_age: Duration,
) -> ReloadWaitStatus {
    let mut last_known_pid = None;
    crate::logging::info(&format!(
        "await_reload_handoff: begin socket={} max_age_ms={} state={}",
        socket_path.display(),
        max_age.as_millis(),
        reload_state_summary(max_age)
    ));

    loop {
        match inspect_reload_wait_status(socket_path, max_age, last_known_pid).await {
            ReloadWaitStatus::Waiting { pid } => {
                last_known_pid = pid;
                crate::logging::info(&format!(
                    "await_reload_handoff: waiting for reload event socket={} pid={:?}",
                    socket_path.display(),
                    pid
                ));
                wait_for_reload_handoff_event(pid, socket_path).await;
            }
            other => {
                crate::logging::info(&format!(
                    "await_reload_handoff: completed socket={} result={:?} state={}",
                    socket_path.display(),
                    other,
                    reload_state_summary(max_age)
                ));
                return other;
            }
        }
    }
}

pub async fn wait_for_reload_handoff_event(
    reloading_pid: Option<u32>,
    socket_path: &std::path::Path,
) {
    crate::logging::info(&format!(
        "wait_for_reload_handoff_event: start socket={} pid={:?}",
        socket_path.display(),
        reloading_pid
    ));
    #[cfg(target_os = "linux")]
    {
        let marker_path = reload_marker_path();
        let socket_path = socket_path.to_path_buf();
        let _ = tokio::task::spawn_blocking(move || {
            wait_for_reload_handoff_event_blocking(&marker_path, &socket_path, reloading_pid)
        })
        .await;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (reloading_pid, socket_path);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    crate::logging::info(&format!(
        "wait_for_reload_handoff_event: wake socket={} pid={:?}",
        socket_path.display(),
        reloading_pid
    ));
}

#[cfg(target_os = "linux")]
fn wait_for_reload_handoff_event_blocking(
    marker_path: &std::path::Path,
    socket_path: &std::path::Path,
    reloading_pid: Option<u32>,
) {
    use std::collections::HashSet;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let mut watch_paths: HashSet<std::path::PathBuf> = HashSet::new();
    if let Some(parent) = marker_path.parent() {
        watch_paths.insert(parent.to_path_buf());
    }
    if let Some(parent) = socket_path.parent() {
        watch_paths.insert(parent.to_path_buf());
    }
    if let Some(pid) = reloading_pid {
        let proc_path = std::path::PathBuf::from(format!("/proc/{pid}"));
        if proc_path.exists() {
            watch_paths.insert(proc_path);
        }
    }

    if watch_paths.is_empty() {
        crate::logging::warn("wait_for_reload_handoff_event_blocking: no watch paths available");
        return;
    }

    crate::logging::info(&format!(
        "wait_for_reload_handoff_event_blocking: marker={} socket={} pid={:?} watch_paths={:?}",
        marker_path.display(),
        socket_path.display(),
        reloading_pid,
        watch_paths
    ));

    unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd < 0 {
            return;
        }

        let mask = libc::IN_CREATE
            | libc::IN_MOVED_TO
            | libc::IN_ATTRIB
            | libc::IN_MODIFY
            | libc::IN_CLOSE_WRITE
            | libc::IN_DELETE
            | libc::IN_MOVE_SELF
            | libc::IN_DELETE_SELF;

        let mut has_watch = false;
        for path in watch_paths {
            let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
                continue;
            };
            if libc::inotify_add_watch(fd, path.as_ptr(), mask) >= 0 {
                has_watch = true;
            }
        }

        if !has_watch {
            crate::logging::warn(
                "wait_for_reload_handoff_event_blocking: failed to register any inotify watches",
            );
            let _ = libc::close(fd);
            return;
        }

        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };

        loop {
            let ready = libc::poll(&mut poll_fd, 1, -1);
            if ready > 0 && (poll_fd.revents & libc::POLLIN) != 0 {
                let mut buf = [0u8; 512];
                let _ = libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len());
                crate::logging::info(
                    "wait_for_reload_handoff_event_blocking: observed filesystem/process event",
                );
                break;
            }
            if ready < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                crate::logging::warn(&format!(
                    "wait_for_reload_handoff_event_blocking: poll failed: {}",
                    err
                ));
                break;
            }
        }

        let _ = libc::close(fd);
    }
}

#[derive(Clone, Debug)]
pub struct ReloadSignal {
    pub hash: String,
    pub triggering_session: Option<String>,
    pub prefer_selfdev_binary: bool,
    pub request_id: String,
}

#[derive(Clone, Debug)]
pub struct ReloadAck {
    pub hash: String,
    pub request_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReloadPhase {
    Starting,
    SocketReady,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReloadState {
    pub request_id: String,
    pub hash: String,
    pub phase: ReloadPhase,
    pub pid: u32,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ReloadState {
    fn path() -> PathBuf {
        reload_marker_path()
    }

    pub(crate) fn write(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = crate::storage::ensure_dir(parent);
        }
        let _ = crate::storage::write_json(&path, self);
    }

    pub fn load() -> Option<Self> {
        let path = Self::path();
        if !path.exists() {
            return None;
        }
        crate::storage::read_json(&path).ok()
    }
}

pub fn reload_state_summary(max_age: Duration) -> String {
    match recent_reload_state(max_age) {
        Some(state) => format!(
            "request={} hash={} phase={:?} pid={} detail={}",
            state.request_id,
            state.hash,
            state.phase,
            state.pid,
            state.detail.unwrap_or_else(|| "<none>".to_string())
        ),
        None => "no recent reload state".to_string(),
    }
}

type ReloadSignalChannel = (
    tokio::sync::watch::Sender<Option<ReloadSignal>>,
    tokio::sync::watch::Receiver<Option<ReloadSignal>>,
);

type ReloadAckChannel = (
    tokio::sync::watch::Sender<Option<ReloadAck>>,
    tokio::sync::watch::Receiver<Option<ReloadAck>>,
);

/// Global reload signal channel. The selfdev tool and debug commands fire this;
/// the server awaits it instead of polling the filesystem.
static RELOAD_SIGNAL: std::sync::OnceLock<ReloadSignalChannel> = std::sync::OnceLock::new();

static RELOAD_ACK: std::sync::OnceLock<ReloadAckChannel> = std::sync::OnceLock::new();

pub(super) fn reload_signal() -> &'static ReloadSignalChannel {
    RELOAD_SIGNAL.get_or_init(|| tokio::sync::watch::channel(None))
}

#[cfg(test)]
pub(crate) fn subscribe_reload_signal_for_tests()
-> tokio::sync::watch::Receiver<Option<ReloadSignal>> {
    reload_signal().1.clone()
}

pub(super) fn reload_ack() -> &'static ReloadAckChannel {
    RELOAD_ACK.get_or_init(|| tokio::sync::watch::channel(None))
}

/// Send a reload signal to the server (called by selfdev tool / debug commands).
pub fn send_reload_signal(
    hash: String,
    triggering_session: Option<String>,
    prefer_selfdev_binary: bool,
) -> String {
    let request_id = crate::id::new_id("reload");
    crate::logging::info(&format!(
        "send_reload_signal: request={} hash={} triggering_session={:?} prefer_selfdev_binary={} current_pid={}",
        request_id,
        hash,
        triggering_session,
        prefer_selfdev_binary,
        std::process::id()
    ));
    let (tx, _) = reload_signal();
    let _ = tx.send(Some(ReloadSignal {
        hash,
        triggering_session,
        prefer_selfdev_binary,
        request_id: request_id.clone(),
    }));
    request_id
}

pub fn acknowledge_reload_signal(signal: &ReloadSignal) {
    crate::logging::info(&format!(
        "acknowledge_reload_signal: request={} hash={} triggering_session={:?} prefer_selfdev_binary={}",
        signal.request_id, signal.hash, signal.triggering_session, signal.prefer_selfdev_binary
    ));
    let (tx, _) = reload_ack();
    let _ = tx.send(Some(ReloadAck {
        hash: signal.hash.clone(),
        request_id: signal.request_id.clone(),
    }));
}

pub async fn wait_for_reload_ack(
    request_id: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<ReloadAck> {
    let mut rx = reload_ack().1.clone();
    let started = std::time::Instant::now();
    crate::logging::info(&format!(
        "wait_for_reload_ack: waiting request={} timeout_ms={}",
        request_id,
        timeout.as_millis()
    ));

    if let Some(ack) = rx.borrow_and_update().clone()
        && ack.request_id == request_id
    {
        crate::logging::info(&format!(
            "wait_for_reload_ack: immediate ack request={} after {}ms",
            request_id,
            started.elapsed().as_millis()
        ));
        return Ok(ack);
    }

    let request_id = request_id.to_string();
    tokio::time::timeout(timeout, async move {
        loop {
            rx.changed()
                .await
                .map_err(|_| anyhow::anyhow!("reload acknowledgement channel closed"))?;
            if let Some(ack) = rx.borrow_and_update().clone()
                && ack.request_id == request_id
            {
                crate::logging::info(&format!(
                    "wait_for_reload_ack: received ack request={} after {}ms",
                    request_id,
                    started.elapsed().as_millis()
                ));
                return Ok(ack);
            }
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "timed out waiting for reload acknowledgement after {}ms (state={})",
            started.elapsed().as_millis(),
            reload_state_summary(Duration::from_secs(60))
        )
    })?
}


#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set_runtime_dir(path: &std::path::Path) -> Self {
            let key = "JCODE_RUNTIME_DIR";
            let old = std::env::var_os(key);
            crate::env::set_var(key, path);
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(old) = &self.old {
                crate::env::set_var(self.key, old);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inspect_reload_wait_status_returns_failed_with_marker_detail() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::set_runtime_dir(temp.path());

        write_reload_state(
            "req-test",
            "hash-test",
            ReloadPhase::Failed,
            Some("reload failed for test".to_string()),
        );

        let status = inspect_reload_wait_status(
            &temp.path().join("jcode.sock"),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert_eq!(
            status,
            ReloadWaitStatus::Failed(Some("reload failed for test".to_string()))
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn inspect_reload_wait_status_returns_ready_for_socket_ready_marker() {
        let _lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::set_runtime_dir(temp.path());

        write_reload_state(
            "req-ready",
            "hash-ready",
            ReloadPhase::SocketReady,
            Some("ready for handoff".to_string()),
        );

        let status = inspect_reload_wait_status(
            &temp.path().join("jcode.sock"),
            Duration::from_secs(5),
            None,
        )
        .await;

        assert_eq!(status, ReloadWaitStatus::Ready);
    }
}
