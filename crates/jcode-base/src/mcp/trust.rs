//! Trust store for project-local MCP configs (issue #62).
//!
//! Project-local `.jcode/mcp.json` (and the compatibility-fallback
//! `.claude/mcp.json`) declare commands jcode is willing to execute on
//! behalf of the project. In a freshly-cloned repository those commands are
//! effectively executable configuration shipped by an unverified author.
//!
//! When `JCODE_REQUIRE_MCP_TRUST=1` is set (auto-set by `--safe-eval`), the
//! MCP loader consults this trust store before loading project-local entries.
//! The store lives at `~/.jcode/mcp_trust.json` and records the SHA-256 of
//! the on-disk file content keyed by the canonical absolute path. A change
//! to the file content invalidates the trust entry and forces a re-approval.
//!
//! The companion CLI commands are:
//!
//!   jcode mcp trust   <path>   # mark the current content of <path> trusted
//!   jcode mcp revoke  <path>   # remove the entry
//!   jcode mcp list             # show all trust entries
//!
//! No interactive TUI prompt is implemented in this PR — the issue's
//! "auditable trust decision" requirement is met by an explicit out-of-band
//! command. A future PR can add an inline "Trust this MCP config? [y/N]"
//! prompt that calls into `mark_trusted` here.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpTrustStore {
    /// Map of canonical absolute path string -> SHA-256 hex of last-trusted content.
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
}

fn store_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("mcp_trust.json"))
}

fn canonicalize_for_key(path: &Path) -> Option<String> {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

/// SHA-256 hex of the file's contents, or None if the file is unreadable.
pub fn content_hash_of(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

impl McpTrustStore {
    pub fn load() -> Self {
        let Ok(path) = store_path() else {
            return Self::default();
        };
        if !path.exists() {
            return Self::default();
        }
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = store_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn is_trusted(&self, path: &Path) -> bool {
        let Some(key) = canonicalize_for_key(path) else {
            return false;
        };
        let Some(expected) = self.entries.get(&key) else {
            return false;
        };
        let Some(actual) = content_hash_of(path) else {
            return false;
        };
        &actual == expected
    }

    /// Insert or update the trust entry for `path` with its current content
    /// hash. Returns the recorded hash.
    pub fn mark_trusted(&mut self, path: &Path) -> Result<String> {
        let key = canonicalize_for_key(path)
            .with_context(|| format!("cannot canonicalize {}", path.display()))?;
        let hash =
            content_hash_of(path).with_context(|| format!("cannot read {}", path.display()))?;
        self.entries.insert(key, hash.clone());
        Ok(hash)
    }

    pub fn revoke(&mut self, path: &Path) -> Option<String> {
        let key = canonicalize_for_key(path).unwrap_or_else(|| path.to_string_lossy().to_string());
        self.entries.remove(&key)
    }
}

/// Whether the trust gate is active for this process.
pub fn trust_gate_enabled() -> bool {
    matches!(
        std::env::var("JCODE_REQUIRE_MCP_TRUST")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_home() -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().expect("temp");
        crate::env::set_var("JCODE_HOME", temp.path());
        temp
    }

    #[test]
    fn mark_and_check_trust_round_trip() {
        let _lock = crate::storage::lock_test_env();
        let _home = isolated_home();

        let workdir = tempfile::TempDir::new().expect("workdir");
        let cfg = workdir.path().join(".jcode/mcp.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, r#"{"servers":{}}"#).unwrap();

        let mut store = McpTrustStore::default();
        assert!(!store.is_trusted(&cfg));
        let hash = store.mark_trusted(&cfg).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(store.is_trusted(&cfg));

        // Modify the file content. Trust must invalidate.
        std::fs::write(&cfg, r#"{"servers":{"new":{"command":"x"}}}"#).unwrap();
        assert!(
            !store.is_trusted(&cfg),
            "content change must invalidate trust"
        );

        // Re-approve with new content.
        store.mark_trusted(&cfg).unwrap();
        assert!(store.is_trusted(&cfg));

        // Revoke removes it.
        let removed = store.revoke(&cfg);
        assert!(removed.is_some());
        assert!(!store.is_trusted(&cfg));
    }

    #[test]
    fn store_persists_across_load_and_save() {
        let _lock = crate::storage::lock_test_env();
        let _home = isolated_home();

        let workdir = tempfile::TempDir::new().expect("workdir");
        let cfg = workdir.path().join(".jcode/mcp.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "{}").unwrap();

        let mut s1 = McpTrustStore::load();
        s1.mark_trusted(&cfg).unwrap();
        s1.save().unwrap();

        let s2 = McpTrustStore::load();
        assert!(s2.is_trusted(&cfg), "saved trust must survive reload");
    }

    #[test]
    fn trust_gate_enabled_honors_common_truthy_values() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_REQUIRE_MCP_TRUST");

        for v in ["1", "true", "yes", "on"] {
            crate::env::set_var("JCODE_REQUIRE_MCP_TRUST", v);
            assert!(trust_gate_enabled(), "{v} should enable gate");
        }
        for v in ["0", "false", "no", "off", ""] {
            crate::env::set_var("JCODE_REQUIRE_MCP_TRUST", v);
            assert!(!trust_gate_enabled(), "{v} should not enable gate");
        }

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_REQUIRE_MCP_TRUST", prev);
        } else {
            crate::env::remove_var("JCODE_REQUIRE_MCP_TRUST");
        }
    }
}
