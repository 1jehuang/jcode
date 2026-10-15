use super::*;
use std::ffi::OsString;

// current_dir() reports the resolved path, and macOS keeps the temp dir under the
// /var -> /private/var symlink, so cwd tests start from the real temp path.
fn real_temp_dir() -> std::path::PathBuf {
    std::fs::canonicalize(std::env::temp_dir()).unwrap()
}

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    crate::storage::lock_test_env()
}

struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }

    fn remove(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[path = "cases.rs"]
mod cases;

#[path = "tool_image_decay.rs"]
mod tool_image_decay;
