//! JCode-owned Command Code API-key accounts.
//!
//! Command Code keys are long-lived bearer credentials. This store lets users
//! keep several labeled keys and choose an active account without writing them
//! into config.toml. The environment variable remains the highest-precedence
//! credential; a legacy `command-code.env` remains a fallback after the store.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STORE_NAME: &str = "command_code_accounts.json";
const LABEL_PREFIX: &str = "command-code";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandCodeAccount {
    pub label: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandCodeAccounts {
    #[serde(default)]
    pub accounts: Vec<CommandCodeAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

fn store_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(STORE_NAME))
}

fn load_from(path: &Path) -> Result<CommandCodeAccounts> {
    if !path.exists() {
        return Ok(CommandCodeAccounts::default());
    }
    crate::storage::read_json(path).with_context(|| format!("read {}", path.display()))
}

/// Highest-precedence stored key: the active labeled account.
pub fn active_api_key() -> Option<String> {
    let path = store_path().ok()?;
    let store = load_from(&path).ok()?;
    let selected = store
        .active
        .as_deref()
        .and_then(|active| {
            store
                .accounts
                .iter()
                .find(|account| account.label == active)
        })
        .or_else(|| store.accounts.first())?;
    let key = selected.api_key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

pub fn labels() -> Vec<String> {
    let Ok(path) = store_path() else {
        return Vec::new();
    };
    let Ok(store) = load_from(&path) else {
        return Vec::new();
    };
    store
        .accounts
        .into_iter()
        .map(|account| account.label)
        .collect()
}

pub fn active_label() -> Option<String> {
    let path = store_path().ok()?;
    load_from(&path).ok()?.active
}

/// Add or replace one labeled account and make it active. An unnamed login gets
/// the next unused generated label. A named login replaces exactly that label.
pub fn save_account(label: Option<&str>, api_key: &str) -> Result<String> {
    let key = api_key.trim();
    anyhow::ensure!(!key.is_empty(), "Command Code API key cannot be empty");
    let dir = crate::storage::jcode_dir()?;
    let path = dir.join(STORE_NAME);
    let _lock = crate::auth::account_store::lock_store(&dir, STORE_NAME)?;
    let mut store = load_from(&path)?;
    let label = label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            crate::auth::account_store::next_account_label(
                LABEL_PREFIX,
                store.accounts.iter().map(|account| account.label.as_str()),
            )
        });
    if let Some(existing) = store
        .accounts
        .iter_mut()
        .find(|account| account.label == label)
    {
        existing.api_key = key.to_string();
    } else {
        store.accounts.push(CommandCodeAccount {
            label: label.clone(),
            api_key: key.to_string(),
        });
    }
    store.active = Some(label.clone());
    crate::storage::write_json_secret(&path, &store)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(label)
}

pub fn set_active(label: &str) -> Result<()> {
    let dir = crate::storage::jcode_dir()?;
    let path = dir.join(STORE_NAME);
    let _lock = crate::auth::account_store::lock_store(&dir, STORE_NAME)?;
    let mut store = load_from(&path)?;
    anyhow::ensure!(
        store.accounts.iter().any(|account| account.label == label),
        "No Command Code account with label '{label}'"
    );
    store.active = Some(label.to_string());
    crate::storage::write_json_secret(&path, &store)?;
    Ok(())
}

pub fn remove_account(label: &str) -> Result<()> {
    let dir = crate::storage::jcode_dir()?;
    let path = dir.join(STORE_NAME);
    let _lock = crate::auth::account_store::lock_store(&dir, STORE_NAME)?;
    let mut store = load_from(&path)?;
    let before = store.accounts.len();
    store.accounts.retain(|account| account.label != label);
    anyhow::ensure!(
        store.accounts.len() != before,
        "No Command Code account with label '{label}'"
    );
    if store.active.as_deref() == Some(label) {
        store.active = store.accounts.first().map(|account| account.label.clone());
    }
    crate::storage::write_json_secret(&path, &store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HomeGuard {
        previous: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(path: &Path) -> Self {
            let lock = crate::storage::lock_test_env();
            let previous = std::env::var_os("JCODE_HOME");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
            }
            crate::env::set_var("JCODE_HOME", path);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    #[test]
    fn labeled_accounts_are_unique_and_switchable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::new(temp.path());
        let first = save_account(Some("work"), "key-work").expect("save work");
        let second = save_account(None, "key-personal").expect("save personal");
        assert_eq!(first, "work");
        assert_ne!(second, "work");
        assert_eq!(active_api_key().as_deref(), Some("key-personal"));
        set_active("work").expect("switch");
        assert_eq!(active_api_key().as_deref(), Some("key-work"));
        let mut labels = labels();
        labels.sort();
        assert_eq!(labels.len(), 2);
        remove_account("work").expect("remove");
        assert_eq!(active_api_key().as_deref(), Some("key-personal"));
    }

    #[test]
    fn environment_key_is_not_written_to_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::new(temp.path());
        crate::env::set_var("COMMAND_CODE_API_KEY", "env-key");
        save_account(Some("work"), "stored-key").expect("save");
        assert_eq!(active_api_key().as_deref(), Some("stored-key"));
        crate::env::remove_var("COMMAND_CODE_API_KEY");
    }

    #[test]
    fn profile_resolution_uses_active_account_then_environment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::new(temp.path());
        save_account(Some("work"), "stored-work").expect("save work");
        save_account(Some("personal"), "stored-personal").expect("save personal");
        set_active("work").expect("switch");
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(
            crate::provider_catalog::COMMAND_CODE_PROFILE,
        );
        assert_eq!(
            super::super::profile_credentials::load_openai_compatible_profile_api_key(&resolved)
                .as_deref(),
            Some("stored-work")
        );
        crate::env::set_var("COMMAND_CODE_API_KEY", "env-key");
        assert_eq!(
            super::super::profile_credentials::load_openai_compatible_profile_api_key(&resolved)
                .as_deref(),
            Some("env-key")
        );
        crate::env::remove_var("COMMAND_CODE_API_KEY");
    }

    #[test]
    fn corrupt_store_is_not_overwritten_by_save() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::new(temp.path());
        let path = temp.path().join(STORE_NAME);
        std::fs::write(&path, b"not json").expect("write corrupt store");
        let error = save_account(Some("work"), "stored-key").expect_err("must not overwrite");
        assert!(error.to_string().contains("read"), "{error}");
        assert_eq!(
            std::fs::read(&path).expect("read store"),
            b"not json",
            "corrupt store must be preserved for recovery"
        );
    }

    #[test]
    fn shared_runtime_key_resolution_uses_active_account() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = HomeGuard::new(temp.path());
        crate::env::remove_var("COMMAND_CODE_API_KEY");
        save_account(Some("work"), "stored-key").expect("save");
        assert_eq!(
            super::super::profile_credentials::load_runtime_api_key(
                "COMMAND_CODE_API_KEY",
                "command-code.env"
            )
            .as_deref(),
            Some("stored-key")
        );
        crate::env::set_var("COMMAND_CODE_API_KEY", "env-key");
        assert_eq!(
            super::super::profile_credentials::load_runtime_api_key(
                "COMMAND_CODE_API_KEY",
                "command-code.env"
            )
            .as_deref(),
            Some("env-key")
        );
        crate::env::remove_var("COMMAND_CODE_API_KEY");
    }
}
