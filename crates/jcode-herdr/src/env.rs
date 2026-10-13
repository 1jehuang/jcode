//! HERDR environment variable capture.
//!
//! When jcode runs inside a HERDR pane, HERDR injects these variables:
//! - `HERDR_ENV=1` — signals HERDR is active
//! - `HERDR_PANE_ID` — pane identifier (e.g. `w1:p1`)
//! - `HERDR_SOCKET_PATH` — path to HERDR's Unix domain socket
//! - `HERDR_BIN_PATH` — path to the HERDR CLI binary
//! - `HERDR_WORKSPACE_ID` — workspace identifier
//! - `HERDR_TAB_ID` — tab identifier

use std::path::PathBuf;

/// Captured HERDR environment. All fields are `None` when not running
/// inside a HERDR pane.
#[derive(Debug, Clone)]
pub struct HerdrEnv {
    /// Whether HERDR is active (`HERDR_ENV=1`).
    pub active: bool,
    /// Pane identifier (e.g. `w1:p1`).
    pub pane_id: Option<String>,
    /// Path to HERDR's Unix domain socket.
    pub socket_path: Option<PathBuf>,
    /// Path to the HERDR CLI binary.
    pub bin_path: Option<PathBuf>,
    /// Workspace identifier.
    pub workspace_id: Option<String>,
    /// Tab identifier.
    pub tab_id: Option<String>,
}

impl HerdrEnv {
    /// Capture HERDR environment from process environment variables.
    pub fn capture() -> Self {
        let active = std::env::var("HERDR_ENV").ok().as_deref() == Some("1");
        Self {
            active,
            pane_id: std::env::var("HERDR_PANE_ID").ok(),
            socket_path: std::env::var("HERDR_SOCKET_PATH").ok().map(PathBuf::from),
            bin_path: std::env::var("HERDR_BIN_PATH").ok().map(PathBuf::from),
            workspace_id: std::env::var("HERDR_WORKSPACE_ID").ok(),
            tab_id: std::env::var("HERDR_TAB_ID").ok(),
        }
    }

    /// Returns `true` if running inside a HERDR pane with valid identity.
    pub fn is_valid(&self) -> bool {
        self.active && self.pane_id.is_some() && self.socket_path.is_some()
    }

    /// Returns the pane ID, panicking if not in a HERDR pane.
    pub fn pane_id(&self) -> &str {
        self.pane_id
            .as_deref()
            .expect("HERDR_PANE_ID not set; call is_valid() first")
    }

    /// Returns the socket path, panicking if not in a HERDR pane.
    pub fn socket_path(&self) -> &PathBuf {
        self.socket_path
            .as_ref()
            .expect("HERDR_SOCKET_PATH not set; call is_valid() first")
    }
}

impl Default for HerdrEnv {
    fn default() -> Self {
        Self::capture()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn capture_inactive_when_no_env() {
        let _env_lock = crate::env::test_support::lock_test_env();
        unsafe {
            env::remove_var("HERDR_ENV");
            env::remove_var("HERDR_PANE_ID");
        }
        let env = HerdrEnv::capture();
        assert!(!env.active);
        assert!(!env.is_valid());
    }

    #[test]
    fn capture_active_with_valid_pane() {
        let _env_lock = crate::env::test_support::lock_test_env();
        // Save existing values to restore after test.
        let saved_env = std::env::var("HERDR_ENV").ok();
        let saved_pane = std::env::var("HERDR_PANE_ID").ok();
        let saved_sock = std::env::var("HERDR_SOCKET_PATH").ok();

        unsafe {
            env::set_var("HERDR_ENV", "1");
            env::set_var("HERDR_PANE_ID", "w1:p1");
            env::set_var("HERDR_SOCKET_PATH", "/tmp/herdr.sock");
        }
        let env = HerdrEnv::capture();
        assert!(env.active);
        assert!(env.is_valid());
        assert_eq!(env.pane_id(), "w1:p1");
        assert_eq!(env.socket_path(), &PathBuf::from("/tmp/herdr.sock"));

        // Restore original values.
        match saved_env {
            Some(v) => unsafe { env::set_var("HERDR_ENV", v) },
            None => unsafe { env::remove_var("HERDR_ENV") },
        }
        match saved_pane {
            Some(v) => unsafe { env::set_var("HERDR_PANE_ID", v) },
            None => unsafe { env::remove_var("HERDR_PANE_ID") },
        }
        match saved_sock {
            Some(v) => unsafe { env::set_var("HERDR_SOCKET_PATH", v) },
            None => unsafe { env::remove_var("HERDR_SOCKET_PATH") },
        }
    }

    #[test]
    fn capture_inactive_when_wrong_value() {
        let _env_lock = crate::env::test_support::lock_test_env();
        unsafe {
            env::set_var("HERDR_ENV", "0");
        }
        let env = HerdrEnv::capture();
        assert!(!env.active);
        unsafe {
            env::remove_var("HERDR_ENV");
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Process-wide lock serializing tests that mutate `HERDR_*`
    /// environment variables. Mutating process env under libtest's
    /// parallel workers is undefined behavior (data race): it has
    /// been observed to hang the libtest console waiting for
    /// CompletedTest events and to fail assertions on captured
    /// values. Every test that touches `HERDR_*` keys must hold
    /// this guard for its entire body.
    pub(crate) fn lock_test_env() -> MutexGuard<'static, ()> {
        static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
