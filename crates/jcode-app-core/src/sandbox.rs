//! Issue #110: sandbox root resolution.
//!
//! Reads the active filesystem sandbox root from environment, set by
//! the `--sandbox-root <DIR>` CLI flag (see `cli::startup`). Tools
//! pass this into `ToolContext::sandbox_root` so that
//! `resolve_path_checked` can reject paths that escape the tree.

use std::path::PathBuf;

/// Return the currently configured sandbox root, if any.
///
/// Reads `JCODE_SANDBOX_ROOT` from process env. Empty / whitespace
/// values are treated as unset.
pub fn current_sandbox_root() -> Option<PathBuf> {
    let raw = std::env::var("JCODE_SANDBOX_ROOT").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_when_unset() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_SANDBOX_ROOT");
        crate::env::remove_var("JCODE_SANDBOX_ROOT");
        assert_eq!(current_sandbox_root(), None);
        if let Some(p) = prev {
            crate::env::set_var("JCODE_SANDBOX_ROOT", p);
        }
    }

    #[test]
    fn returns_path_when_set() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_SANDBOX_ROOT");
        crate::env::set_var("JCODE_SANDBOX_ROOT", "/tmp/safe");
        assert_eq!(current_sandbox_root(), Some(PathBuf::from("/tmp/safe")));
        if let Some(p) = prev {
            crate::env::set_var("JCODE_SANDBOX_ROOT", p);
        } else {
            crate::env::remove_var("JCODE_SANDBOX_ROOT");
        }
    }

    #[test]
    fn empty_value_treated_as_unset() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_SANDBOX_ROOT");
        crate::env::set_var("JCODE_SANDBOX_ROOT", "  ");
        assert_eq!(current_sandbox_root(), None);
        if let Some(p) = prev {
            crate::env::set_var("JCODE_SANDBOX_ROOT", p);
        } else {
            crate::env::remove_var("JCODE_SANDBOX_ROOT");
        }
    }
}
