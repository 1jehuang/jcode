use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::McpConfig;

const TRUST_STORE_VERSION: u32 = 1;
const TRUST_STORE_FILE: &str = "mcp-project-trust.json";

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

#[derive(Debug, Serialize, Deserialize)]
struct ProjectMcpTrustStore {
    version: u32,
    #[serde(default)]
    projects: BTreeMap<String, String>,
}

impl Default for ProjectMcpTrustStore {
    fn default() -> Self {
        Self {
            version: TRUST_STORE_VERSION,
            projects: BTreeMap::new(),
        }
    }
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

fn trust_store_path() -> PathBuf {
    crate::storage::durable_state_dir().join(TRUST_STORE_FILE)
}

fn canonical_project_root(project_root: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(project_root).with_context(|| {
        format!(
            "failed to resolve project directory {}",
            project_root.display()
        )
    })?;
    if !canonical.is_dir() {
        anyhow::bail!("project path is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn project_key(project_root: &Path) -> String {
    project_root.to_string_lossy().into_owned()
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

fn load_store() -> ProjectMcpTrustStore {
    let path = trust_store_path();
    if !path.exists() {
        return ProjectMcpTrustStore::default();
    }
    match crate::storage::read_json::<ProjectMcpTrustStore>(&path) {
        Ok(store) if store.version == TRUST_STORE_VERSION => store,
        Ok(store) => {
            crate::logging::warn(&format!(
                "MCP trust store version {} is unsupported; project-local MCP remains blocked",
                store.version
            ));
            ProjectMcpTrustStore::default()
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Failed to read MCP trust store {}; project-local MCP remains blocked: {error}",
                path.display()
            ));
            ProjectMcpTrustStore::default()
        }
    }
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
        .into_iter()
        .map(|(_, review)| review)
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

pub fn project_mcp_is_trusted(review: &ProjectMcpReview) -> bool {
    load_store()
        .projects
        .get(&project_key(&review.project_root))
        .is_some_and(|fingerprint| fingerprint == &review.fingerprint)
}

pub fn trust_project_mcp(review: &ProjectMcpReview) -> Result<()> {
    let mut store = load_store();
    store.projects.insert(
        project_key(&review.project_root),
        review.fingerprint.clone(),
    );
    crate::storage::write_json(&trust_store_path(), &store)
}

pub fn revoke_project_mcp(project_root: &Path) -> Result<bool> {
    let project_root = canonical_project_root(project_root)?;
    let mut store = load_store();
    let removed = store.projects.remove(&project_key(&project_root)).is_some();
    if removed {
        crate::storage::write_json(&trust_store_path(), &store)?;
    }
    Ok(removed)
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

        assert!(project_mcp_review(project.path()).unwrap().is_none());
        assert!(
            McpConfig::load_for_dir(Some(project.path()))
                .servers
                .is_empty()
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
