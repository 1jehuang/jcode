use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::McpConfig;

#[derive(Clone, PartialEq, Eq)]
pub struct ProjectMcpServerReview {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProjectMcpReview {
    pub project_root: PathBuf,
    pub fingerprint: String,
    pub servers: Vec<ProjectMcpServerReview>,
}

#[derive(Serialize)]
struct FingerprintServer<'a> {
    command: &'a str,
    args: &'a [String],
    env: BTreeMap<&'a str, &'a str>,
    shared: bool,
    transport: Option<&'a str>,
    url: Option<&'a str>,
    headers: BTreeMap<&'a str, &'a str>,
    enabled: Option<bool>,
    disabled: Option<bool>,
}

fn canonical_project_root(project_root: &Path) -> Result<PathBuf> {
    crate::project_trust::project_root(project_root)
}

fn stable_map(map: &HashMap<String, String>) -> BTreeMap<&str, &str> {
    map.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

fn fingerprint(config: &McpConfig) -> Result<String> {
    let servers: BTreeMap<&str, FingerprintServer<'_>> = config
        .servers
        .iter()
        .map(|(name, server)| {
            (
                name.as_str(),
                FingerprintServer {
                    command: &server.command,
                    args: &server.args,
                    env: stable_map(&server.env),
                    shared: server.shared,
                    transport: server.transport.as_deref(),
                    url: server.url.as_deref(),
                    headers: stable_map(&server.headers),
                    enabled: server.enabled,
                    disabled: server.disabled,
                },
            )
        })
        .collect();
    let encoded = serde_json::to_vec(&servers)?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

pub(super) fn review_project_config(
    project_root: &Path,
    config: &McpConfig,
) -> Result<Option<ProjectMcpReview>> {
    if config.servers.is_empty() {
        return Ok(None);
    }
    let project_root = canonical_project_root(project_root)?;
    let servers = config
        .servers
        .iter()
        .map(|(name, config)| {
            (
                name.clone(),
                ProjectMcpServerReview {
                    name: name.clone(),
                    command: config.command.clone(),
                    args: config.args.clone(),
                    env: config
                        .env
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    Ok(Some(ProjectMcpReview {
        project_root,
        fingerprint: fingerprint(config)?,
        servers,
    }))
}

pub fn project_mcp_review(project_root: &Path) -> Result<Option<ProjectMcpReview>> {
    let mut config = McpConfig::load_project_locals(project_root);
    config.servers.retain(|_, server| server.is_stdio());
    config.expand_environment_variables();
    config.servers.retain(|_, server| server.is_stdio());
    review_project_config(project_root, &config)
}

/// Digest stored in the shared project trust store for `review`.
fn store_digest(review: &ProjectMcpReview) -> String {
    format!("sha256:{}", review.fingerprint)
}

pub fn project_mcp_is_trusted(review: &ProjectMcpReview) -> bool {
    crate::project_trust::is_trusted(
        &review.project_root,
        crate::project_trust::Scope::Mcp,
        &store_digest(review),
    )
}

pub fn trust_project_mcp(review: &ProjectMcpReview) -> Result<()> {
    crate::project_trust::grant(
        &review.project_root,
        crate::project_trust::Scope::Mcp,
        &store_digest(review),
    )
    .map(drop)
}

pub fn revoke_project_mcp(project_root: &Path) -> Result<bool> {
    crate::project_trust::revoke(project_root, Some(crate::project_trust::Scope::Mcp))
}

fn key_looks_secret(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "AUTH",
    ]
    .iter()
    .any(|word| key.contains(word))
}

/// What the `mcp` scope would let `path`'s project run, for `jcode trust`.
pub fn mcp_scope_review(path: &Path) -> Result<Option<crate::project_trust::ScopeReview>> {
    let Some(review) = project_mcp_review(path)? else {
        return Ok(None);
    };
    let mut redacted = false;
    let mut lines = Vec::new();
    for server in &review.servers {
        use crate::project_trust::safe_terminal_text as safe;
        lines.push(format!("  - {}", safe(&server.name)));
        lines.push(format!("    command: {:?}", safe(&server.command)));
        if !server.args.is_empty() {
            lines.push("    arguments:".to_string());
            lines.extend(
                server
                    .args
                    .iter()
                    .map(|arg| format!("      - {:?}", safe(arg))),
            );
        }
        if !server.env.is_empty() {
            lines.push("    environment:".to_string());
            for (key, value) in &server.env {
                let assignment = format!("{key}={value}");
                let mut shown = crate::message::redact_secrets(&assignment);
                // Opaque values under bare names (`TOKEN`, `PASSWORD`, ...)
                // have no recognisable shape, so also judge by the key.
                if shown == assignment && key_looks_secret(key) {
                    shown = format!("{key}=[redacted]");
                }
                redacted |= shown != assignment;
                lines.push(format!("      - {:?}", safe(&shown)));
            }
        }
    }
    Ok(Some(crate::project_trust::ScopeReview {
        scope: crate::project_trust::Scope::Mcp,
        digest: store_digest(&review),
        lines,
        item_count: review.servers.len(),
        redacted,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => crate::env::set_var(self.key, value),
                None => crate::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn project_config_requires_exact_executable_trust() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        let config_path = project.path().join(".mcp.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers":{"repo-tool":{"command":"first-command"}}}"#,
        )
        .unwrap();

        let review = project_mcp_review(project.path()).unwrap().unwrap();
        assert!(!project_mcp_is_trusted(&review));
        assert!(
            !McpConfig::load_for_dir(Some(project.path()))
                .servers
                .contains_key("repo-tool")
        );

        trust_project_mcp(&review).unwrap();
        assert!(project_mcp_is_trusted(&review));
        assert_eq!(
            McpConfig::load_for_dir(Some(project.path())).servers["repo-tool"].command,
            "first-command"
        );

        std::fs::write(
            &config_path,
            r#"{"mcpServers":{"repo-tool":{"command":"changed-command"}}}"#,
        )
        .unwrap();
        let changed = project_mcp_review(project.path()).unwrap().unwrap();
        assert_ne!(changed.fingerprint, review.fingerprint);
        assert!(!project_mcp_is_trusted(&changed));
        assert!(
            !McpConfig::load_for_dir(Some(project.path()))
                .servers
                .contains_key("repo-tool")
        );
    }

    #[test]
    fn expanded_environment_changes_invalidate_trust() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let _command = EnvGuard::set("JCODE_MCP_TRUST_TEST_COMMAND", "first-command");
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"repo-tool":{"command":"${JCODE_MCP_TRUST_TEST_COMMAND}"}}}"#,
        )
        .unwrap();

        let review = project_mcp_review(project.path()).unwrap().unwrap();
        trust_project_mcp(&review).unwrap();
        assert_eq!(
            McpConfig::load_for_dir(Some(project.path())).servers["repo-tool"].command,
            "first-command"
        );

        crate::env::set_var("JCODE_MCP_TRUST_TEST_COMMAND", "changed-command");
        let changed = project_mcp_review(project.path()).unwrap().unwrap();
        assert_ne!(changed.fingerprint, review.fingerprint);
        assert!(!project_mcp_is_trusted(&changed));
        assert!(
            !McpConfig::load_for_dir(Some(project.path()))
                .servers
                .contains_key("repo-tool")
        );
    }

    #[test]
    fn blocking_project_overlay_preserves_trusted_global_server() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("mcp.json"),
            r#"{"mcpServers":{"same-name":{"command":"global-command"}}}"#,
        )
        .unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"same-name":{"command":"project-command"}}}"#,
        )
        .unwrap();

        let config = McpConfig::load_for_dir(Some(project.path()));
        assert_eq!(config.servers["same-name"].command, "global-command");
    }

    #[test]
    fn non_stdio_project_entries_do_not_require_trust() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"remote":{"type":"http","url":"https://example.invalid/mcp"}}}"#,
        )
        .unwrap();

        // Remote servers run no local command: nothing to approve, and they
        // keep loading as before.
        assert!(project_mcp_review(project.path()).unwrap().is_none());
        assert!(
            McpConfig::load_for_dir(Some(project.path()))
                .servers
                .contains_key("remote")
        );
    }

    #[test]
    fn revocation_removes_persisted_trust() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = EnvGuard::set("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"repo-tool":{"command":"tool"}}}"#,
        )
        .unwrap();
        let review = project_mcp_review(project.path()).unwrap().unwrap();
        trust_project_mcp(&review).unwrap();

        assert!(revoke_project_mcp(project.path()).unwrap());
        assert!(!project_mcp_is_trusted(&review));
        assert!(!revoke_project_mcp(project.path()).unwrap());
    }
}

#[cfg(test)]
mod shared_store_tests {
    use super::*;

    #[test]
    fn mcp_grant_lands_in_shared_store_under_the_mcp_scope_only() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"t":{"command":"tool"}}}"#,
        )
        .unwrap();
        let review = project_mcp_review(project.path()).unwrap().unwrap();
        trust_project_mcp(&review).unwrap();
        let entries = crate::project_trust::list();
        let result = (
            entries.len(),
            entries[0].1,
            crate::project_trust::is_trusted(
                project.path(),
                crate::project_trust::Scope::Hooks,
                &entries[0].2.digest,
            ),
        );
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        assert_eq!(result.0, 1);
        assert_eq!(result.1, crate::project_trust::Scope::Mcp);
        assert!(!result.2, "granting mcp must not grant hooks");
    }

    #[test]
    fn scope_review_lists_commands_and_redacts_secret_env() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"t":{"command":"tool","args":["--x"],"env":{"API_KEY":"sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789"}}}}"#,
        )
        .unwrap();
        let review = mcp_scope_review(project.path()).unwrap().unwrap();
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        assert_eq!(review.item_count, 1);
        assert!(review.lines.iter().any(|l| l.contains("tool")));
        assert!(review.redacted);
        assert!(review.digest.starts_with("sha256:"));
    }

    #[test]
    fn opaque_values_under_secret_looking_names_are_redacted() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"t":{"command":"tool","env":{"TOKEN":"opaque-value-1","PASSWORD":"hunter2","MODE":"fast"}}}}"#,
        )
        .unwrap();
        let review = mcp_scope_review(project.path()).unwrap().unwrap();
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        let shown = review.lines.join("\n");
        assert!(
            !shown.contains("opaque-value-1") && !shown.contains("hunter2"),
            "{shown}"
        );
        assert!(
            shown.contains("MODE=fast"),
            "non-secret values stay visible: {shown}"
        );
        assert!(review.redacted);
    }
}
