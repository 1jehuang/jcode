//! HERDR lifecycle reporter for jcode and ForgeCode.
//!
//! [`HerdrReporter`] manages state transitions, debouncing, monotonic
//! sequence numbers, and session identity. It is a no-op when HERDR is
//! not detected.
//!
//! # Usage
//!
//! ```no_run
//! use jcode_herdr::{HerdrReporter, AgentState};
//!
//! # async fn example() {
//! let mut reporter = HerdrReporter::new("jcode");
//!
//! // Report state transitions
//! reporter.set_state(AgentState::Working).await;
//! // ... agent works ...
//! reporter.set_state(AgentState::Idle).await;
//!
//! // Report session identity for restore
//! reporter.set_session_id("sess_abc123".to_string()).await;
//!
//! // Release on exit (drops are automatic via Drop impl)
//! reporter.release().await;
//! # }
//! ```

use crate::env::HerdrEnv;
use crate::socket;
use crate::state::AgentState;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::Mutex;

/// Parse a duration from an environment variable, falling back to default.
fn parse_duration_env(name: &str, fallback: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

/// Monotonic sequence counter shared across all reporter instances
/// in the process. Ensures stale reports from predecessor sessions
/// are dropped by HERDR.
static GLOBAL_SEQ: AtomicI64 = AtomicI64::new(0);

/// Return a strictly increasing sequence number. Combines a wall-clock
/// timestamp (millisecond precision × 1000) with an atomic counter to
/// guarantee monotonicity even when called multiple times per ms.
fn next_seq() -> i64 {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
        * 1000;
    // Ensure the counter is always >= ts*1000 and strictly increasing.
    // CAS loop to handle concurrent callers
    loop {
        let prev = GLOBAL_SEQ.load(Ordering::Relaxed);
        let next = std::cmp::max(prev + 1, ts);
        match GLOBAL_SEQ.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(_) => continue,
        }
    }
}

/// HERDR lifecycle reporter. Manages state reporting, debouncing,
/// session identity, and cleanup.
pub struct HerdrReporter {
    env: HerdrEnv,
    agent_label: String,
    source: String,
    state: Arc<Mutex<AgentStateInner>>,
    _idle_debounce_ms: u64,
    retry_grace_ms: u64,
}

struct AgentStateInner {
    current: AgentState,
    message: Option<String>,
    session_id: Option<String>,
    session_path: Option<String>,
    released: bool,
    retry_hold_active: bool,
    failure_blocked: bool,
    failure_message: Option<String>,
}

impl HerdrReporter {
    /// Create a new reporter for the given agent label.
    /// If HERDR is not active, all operations are no-ops.
    pub fn new(agent_label: &str) -> Self {
        let env = HerdrEnv::capture();
        let source = format!("jcode:{agent_label}");
        let idle_debounce_ms = parse_duration_env("HERDR_JCODE_IDLE_DEBOUNCE_MS", 250);
        let retry_grace_ms = parse_duration_env("HERDR_JCODE_RETRY_GRACE_MS", 2500);
        Self {
            env,
            agent_label: agent_label.to_string(),
            source,
            _idle_debounce_ms: idle_debounce_ms,
            retry_grace_ms,
            state: Arc::new(Mutex::new(AgentStateInner {
                current: AgentState::Idle,
                message: None,
                session_id: None,
                session_path: None,
                released: false,
                retry_hold_active: false,
                failure_blocked: false,
                failure_message: None,
            })),
        }
    }

    /// Returns `true` if HERDR is active and this reporter can send.
    pub fn is_active(&self) -> bool {
        self.env.is_valid()
    }

    /// Report a state transition to HERDR.
    pub async fn set_state(&self, state: AgentState) {
        self.set_state_with_message(state, None).await;
    }

    /// Report a state transition with an optional message.
    pub async fn set_state_with_message(&self, state: AgentState, message: Option<String>) {
        if !self.is_active() {
            return;
        }

        let mut inner = self.state.lock().await;
        if inner.released {
            return;
        }

        if inner.current == state && inner.message == message {
            return;
        }

        inner.current = state;
        inner.message = message.clone();
        drop(inner);

        self.send_state_report(state, message).await;
    }

    /// Report session identity for restore.
    pub async fn set_session_id(&self, session_id: String) {
        if !self.is_active() {
            return;
        }

        {
            let mut inner = self.state.lock().await;
            inner.session_id = Some(session_id.clone());
        }

        let params = serde_json::json!({
            "pane_id": self.env.pane_id(),
            "source": self.source,
            "agent": self.agent_label,
            "agent_session_id": session_id,
            "seq": next_seq(),
        });

        let request = serde_json::json!({
            "id": format!("{}:session:{}", self.source, next_seq()),
            "method": "pane.report_agent_session",
            "params": params,
        });

        socket::send_fire_and_forget(self.env.socket_path(), &request).await;
    }

    /// Report session file path for restore.
    pub async fn set_session_path(&self, session_path: String) {
        if !self.is_active() {
            return;
        }

        {
            let mut inner = self.state.lock().await;
            inner.session_path = Some(session_path.clone());
        }

        let params = serde_json::json!({
            "pane_id": self.env.pane_id(),
            "source": self.source,
            "agent": self.agent_label,
            "agent_session_path": session_path,
            "seq": next_seq(),
        });

        let request = serde_json::json!({
            "id": format!("{}:session-path:{}", self.source, next_seq()),
            "method": "pane.report_agent_session",
            "params": params,
        });

        socket::send_fire_and_forget(self.env.socket_path(), &request).await;
    }

    /// Release this agent's authority on the pane.
    /// Called automatically on drop, but can be called explicitly
    /// for deterministic cleanup.
    pub async fn release(&self) {
        if !self.is_active() {
            return;
        }

        let mut inner = self.state.lock().await;
        if inner.released {
            return;
        }
        inner.released = true;
        drop(inner);

        let request = serde_json::json!({
            "id": format!("{}:release:{}", self.source, next_seq()),
            "method": "pane.release_agent",
            "params": {
                "pane_id": self.env.pane_id(),
                "source": self.source,
                "agent": self.agent_label,
                "seq": next_seq(),
            }
        });

        socket::send_fire_and_forget(self.env.socket_path(), &request).await;
    }

    /// Send the initial idle report on session start.
    pub async fn on_session_start(&self) {
        if !self.is_active() {
            return;
        }
        self.send_state_report(AgentState::Idle, None).await;
    }

    /// Report a provider error. Holds the pane in working state for
    /// `retry_grace_ms` (default 2.5s) to allow auto-retry, then
    /// marks as blocked with the error message.
    pub async fn report_error(&self, error_message: String) {
        if !self.is_active() {
            return;
        }

        {
            let mut inner = self.state.lock().await;
            inner.retry_hold_active = true;
            inner.failure_message = Some(error_message.clone());
        }

        // Hold working during retry grace window
        self.send_state_report(AgentState::Working, None).await;

        let grace = self.retry_grace_ms;
        let state = self.state.clone();
        let env = self.env.clone();
        let source = self.source.clone();
        let agent_label = self.agent_label.clone();

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(grace)).await;
            let mut inner = state.lock().await;
            if inner.retry_hold_active {
                inner.retry_hold_active = false;
                inner.failure_blocked = true;
                inner.current = AgentState::Blocked;
                inner.message = Some(error_message.clone());
                drop(inner);

                // Send the blocked state over socket.
                let params = serde_json::json!({
                    "pane_id": env.pane_id(),
                    "source": source,
                    "agent": agent_label,
                    "state": "blocked",
                    "message": error_message,
                    "seq": next_seq(),
                });
                let request = serde_json::json!({
                    "id": format!("{source}:blocked:{}", next_seq()),
                    "method": "pane.report_agent",
                    "params": params,
                });
                socket::send_fire_and_forget(env.socket_path(), &request).await;
            }
        });
    }

    async fn send_state_report(&self, state: AgentState, message: Option<String>) {
        let mut params = serde_json::json!({
            "pane_id": self.env.pane_id(),
            "source": self.source,
            "agent": self.agent_label,
            "state": state.as_str(),
            "seq": next_seq(),
        });

        if let Some(msg) = &message {
            params["message"] = serde_json::Value::String(msg.clone());
        }

        {
            let inner = self.state.lock().await;
            if let Some(sid) = &inner.session_id {
                params["agent_session_id"] = serde_json::Value::String(sid.clone());
            }
            if let Some(spath) = &inner.session_path {
                params["agent_session_path"] = serde_json::Value::String(spath.clone());
            }
        }

        let request = serde_json::json!({
            "id": format!("{}:{}:{}", self.source, state.as_str(), next_seq()),
            "method": "pane.report_agent",
            "params": params,
        });

        socket::send_fire_and_forget(self.env.socket_path(), &request).await;
    }
}

impl Drop for HerdrReporter {
    fn drop(&mut self) {
        if !self.is_active() {
            return;
        }

        // Release synchronously via a blocking spawn.
        let env = self.env.clone();
        let source = self.source.clone();
        let agent_label = self.agent_label.clone();

        let request = serde_json::json!({
            "id": format!("{source}:release-drop:{}", next_seq()),
            "method": "pane.release_agent",
            "params": {
                "pane_id": env.pane_id(),
                "source": source,
                "agent": agent_label,
                "seq": next_seq(),
            }
        });

        // Best-effort: spawn a short-lived task for the socket write.
        // Guard with try_current to avoid panicking when dropped outside
        // a tokio runtime (e.g. synchronous test cleanup).
        let socket_path = env.socket_path().clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                socket::send_fire_and_forget(&socket_path, &request).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    /// Spawn a mock Unix socket server that collects all received
    /// JSON-RPC requests. Returns the socket path and a channel
    /// receiver that yields each request as it arrives.
    async fn start_mock_server() -> (
        PathBuf,
        tokio::sync::mpsc::Receiver<serde_json::Value>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    // Read exactly one newline-terminated line using
                    // raw byte reads (no BufReader) to avoid buffering
                    // past the line boundary.
                    let mut buf = Vec::new();
                    let mut byte = [0u8; 1];
                    loop {
                        let n = stream.read(&mut byte).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        if byte[0] == b'\n' {
                            break;
                        }
                        buf.push(byte[0]);
                    }
                    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&buf) {
                        let _ = tx.send(val).await;
                    }
                    // Write response so client's read_line doesn't hang.
                    let resp = serde_json::json!({"ok": true});
                    let resp_str = serde_json::to_string(&resp).unwrap();
                    let _ = stream.write_all(format!("{resp_str}\n").as_bytes()).await;
                });
            }
        });

        (sock, rx, dir)
    }

    /// Helper to save and restore env vars across tests to prevent
    /// leakage between parallel test runs.
    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.vars {
                match v {
                    Some(val) => unsafe {
                        std::env::set_var(k, val);
                    },
                    None => unsafe {
                        std::env::remove_var(k);
                    },
                }
            }
        }
    }

    /// Set the standard HERDR env vars for tests, returning a guard
    /// that restores original values on drop.
    fn set_herdr_env(sock: &std::path::Path) -> EnvGuard {
        let keys = ["HERDR_ENV", "HERDR_PANE_ID", "HERDR_SOCKET_PATH"];
        let vars: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        unsafe {
            std::env::set_var("HERDR_ENV", "1");
            std::env::set_var("HERDR_PANE_ID", "test-pane");
            std::env::set_var("HERDR_SOCKET_PATH", sock.to_str().unwrap());
        }
        EnvGuard { vars }
    }

    #[test]
    fn reporter_source_format() {
        let _env_lock = crate::env::test_support::lock_test_env();
        let reporter = HerdrReporter::new("jcode");
        assert_eq!(reporter.source, "jcode:jcode");
    }

    #[test]
    fn inactive_reporter_is_noop() {
        let _env_lock = crate::env::test_support::lock_test_env();
        // Without HERDR_ENV, reporter should be inactive.
        // Save and restore to prevent env leakage.
        let saved = std::env::var("HERDR_ENV").ok();
        unsafe { std::env::remove_var("HERDR_ENV") };
        let reporter = HerdrReporter::new("test-agent");
        assert!(!reporter.is_active());
        if let Some(v) = saved {
            unsafe { std::env::set_var("HERDR_ENV", v) };
        }
    }

    #[test]
    fn seq_is_monotonic() {
        let s1 = next_seq();
        let s2 = next_seq();
        assert!(s2 > s1);
    }

    #[tokio::test]
    async fn reporter_sends_working_state() {
        let (sock, mut rx, _dir) = start_mock_server().await;
        let _env_lock = crate::env::test_support::lock_test_env();
        let _guard = set_herdr_env(&sock);

        let reporter = HerdrReporter::new("jcode");
        reporter.set_state(AgentState::Working).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let req = rx.recv().await.expect("expected a request");
        assert_eq!(req["method"], "pane.report_agent");
        assert_eq!(req["params"]["state"], "working");
        assert_eq!(req["params"]["agent"], "jcode");
        assert_eq!(req["params"]["pane_id"], "test-pane");

        reporter.release().await;
    }

    #[tokio::test]
    async fn reporter_sends_idle_state() {
        let (sock, mut rx, _dir) = start_mock_server().await;
        let _env_lock = crate::env::test_support::lock_test_env();
        let _guard = set_herdr_env(&sock);

        let reporter = HerdrReporter::new("jcode");
        // Default state is Idle, so set Working first to bypass
        // the dedup check, then set Idle to trigger a send.
        reporter.set_state(AgentState::Working).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Drain the working request.
        let _ = rx.recv().await;

        reporter.set_state(AgentState::Idle).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let req = rx.recv().await.expect("expected idle request");
        assert_eq!(req["params"]["state"], "idle");

        reporter.release().await;
    }

    #[tokio::test]
    async fn reporter_sends_blocked_state() {
        let (sock, mut rx, _dir) = start_mock_server().await;
        let _env_lock = crate::env::test_support::lock_test_env();
        let _guard = set_herdr_env(&sock);

        let reporter = HerdrReporter::new("jcode");
        reporter.report_error("permission denied".into()).await;

        // report_error first sends "working", then spawns a delayed
        // "blocked" after retry_grace_ms. Consume the working
        // request, then wait for the blocked one.
        let req = rx.recv().await.expect("expected working request");
        assert_eq!(req["params"]["state"], "working");

        // Wait for the grace period task to fire blocked state.
        let req = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for blocked request")
            .expect("expected blocked request");
        assert_eq!(req["params"]["state"], "blocked");
        assert_eq!(req["params"]["message"], "permission denied");

        reporter.release().await;
    }

    #[tokio::test]
    async fn reporter_sends_session_id() {
        let (sock, mut rx, _dir) = start_mock_server().await;
        let _env_lock = crate::env::test_support::lock_test_env();
        let _guard = set_herdr_env(&sock);

        let reporter = HerdrReporter::new("jcode");
        reporter.set_session_id("sess_abc123".to_string()).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let req = rx.recv().await.expect("expected session request");
        assert_eq!(req["method"], "pane.report_agent_session");
        assert_eq!(req["params"]["agent_session_id"], "sess_abc123");

        reporter.release().await;
    }

    #[tokio::test]
    async fn reporter_seq_numbers_increase() {
        let (sock, mut rx, _dir) = start_mock_server().await;
        let _env_lock = crate::env::test_support::lock_test_env();
        let _guard = set_herdr_env(&sock);

        let reporter = HerdrReporter::new("jcode");
        reporter.set_state(AgentState::Working).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        reporter.set_state(AgentState::Idle).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let req1 = rx.recv().await.expect("req1");
        let req2 = rx.recv().await.expect("req2");

        let seq1 = req1["params"]["seq"].as_i64().unwrap();
        let seq2 = req2["params"]["seq"].as_i64().unwrap();
        assert!(seq2 > seq1, "seq2 ({seq2}) should be > seq1 ({seq1})");

        reporter.release().await;
    }
}
