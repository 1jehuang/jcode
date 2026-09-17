//! HERDR terminal runtime integration.
//!
//! Provides a global [`jcode_herdr::HerdrReporter`] that the TUI and
//! provider layers use to report lifecycle state to HERDR. All
//! operations are no-ops when jcode is not running inside a HERDR pane.

use jcode_herdr::{AgentState, HerdrReporter};
use std::sync::OnceLock;
use tokio::sync::Mutex;

static REPORTER: OnceLock<Mutex<Option<HerdrReporter>>> = OnceLock::new();

/// Initialize the global HERDR reporter. Safe to call multiple times;
/// only the first call takes effect.
pub fn init(agent_label: &str) {
    let _ = REPORTER.set(Mutex::new(Some(HerdrReporter::new(agent_label))));
}

/// Force-initialize with `HERDR_ENV=1` for testing outside a real pane.
pub fn init_forced(agent_label: &str) {
    unsafe { std::env::set_var("HERDR_ENV", "1"); }
    let _ = REPORTER.set(Mutex::new(Some(HerdrReporter::new(agent_label))));
}

/// Report a state transition to HERDR. No-op if not initialized or
/// not running inside HERDR.
pub async fn report_state(state: AgentState) {
    if let Some(m) = REPORTER.get() {
        let guard = m.lock().await;
        if let Some(reporter) = guard.as_ref() {
            reporter.set_state(state).await;
        }
    }
}

/// Report session identity for restore.
pub async fn report_session_id(session_id: String) {
    if let Some(m) = REPORTER.get() {
        let guard = m.lock().await;
        if let Some(reporter) = guard.as_ref() {
            reporter.set_session_id(session_id).await;
        }
    }
}

/// Send the initial idle report on session start.
pub async fn on_session_start() {
    if let Some(m) = REPORTER.get() {
        let guard = m.lock().await;
        if let Some(reporter) = guard.as_ref() {
            reporter.on_session_start().await;
        }
    }
}

/// Release the agent and shut down the reporter.
pub async fn shutdown() {
    if let Some(m) = REPORTER.get() {
        let mut guard = m.lock().await;
        if let Some(reporter) = guard.take() {
            reporter.release().await;
        }
    }
}

/// Returns `true` if HERDR is active and the reporter is initialized.
pub fn is_active() -> bool {
    REPORTER
        .get()
        .and_then(|m| m.try_lock().ok())
        .and_then(|guard| guard.as_ref().map(|r| r.is_active()))
        .unwrap_or(false)
}
