//! One-use completion receipts for a Claude sign-in in another terminal.
use super::claude;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

tokio::task_local! {
    static VALIDATION_TOKENS: std::cell::RefCell<claude::ClaudeCredentials>;
}

/// Follow only successful refresh commits in this validation task.
pub async fn with_validation_refreshes(
    credentials: claude::ClaudeCredentials,
    validation: impl std::future::Future<Output = Result<()>>,
) -> Result<claude::ClaudeCredentials> {
    VALIDATION_TOKENS
        .scope(std::cell::RefCell::new(credentials), async {
            validation.await?;
            Ok(VALIDATION_TOKENS.with(|tokens| tokens.borrow().clone()))
        })
        .await
}

pub(in crate::auth) fn follow_validation_refresh(
    previous_refresh: &str,
    access: &str,
    refresh: &str,
    expires: i64,
) -> bool {
    matches!(
        VALIDATION_TOKENS.try_with(|tokens| {
            let mut tokens = tokens.borrow_mut();
            if previous_refresh.trim().is_empty() || tokens.refresh_token != previous_refresh {
                return false;
            }
            tokens.access_token = access.into();
            tokens.refresh_token = refresh.into();
            tokens.expires_at = expires;
            true
        }),
        Ok(true)
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LoginSource {
    Native,
    File(PathBuf),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompletedLogin {
    source: LoginSource,
    pub fingerprint: String,
    pub saved_account: Option<String>,
    persisted_default: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    attempt: String,
    #[serde(default)]
    claimed: bool,
    finished: bool,
    completed: Option<CompletedLogin>,
}

#[derive(Debug)]
pub struct LoginFlow {
    _directory: tempfile::TempDir,
    pub path: PathBuf,
    pub attempt: String,
}

impl LoginFlow {
    pub fn new() -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("jcode-claude-login-")
            .tempdir()?;
        let path = directory.path().join("result.json");
        let attempt = uuid::Uuid::new_v4().to_string();
        crate::storage::write_json_secret(
            &path,
            &Receipt {
                attempt: attempt.clone(),
                claimed: false,
                finished: false,
                completed: None,
            },
        )?;
        Ok(Self {
            _directory: directory,
            path,
            attempt,
        })
    }

    pub fn cancel(&self) -> Result<()> {
        let _lock = super::account_store::lock_store(self._directory.path(), "result.json")?;
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn result(&self) -> Result<Option<Option<CompletedLogin>>> {
        let receipt = read_receipt(&self.path)?;
        anyhow::ensure!(
            receipt.attempt == self.attempt,
            "Claude login completion belongs to another attempt"
        );
        Ok(receipt.finished.then_some(receipt.completed))
    }
}

fn read_receipt(path: &Path) -> Result<Receipt> {
    anyhow::ensure!(
        path.is_absolute(),
        "Claude login completion path must be absolute"
    );
    let path = crate::storage::validate_external_auth_file(path)?;
    anyhow::ensure!(
        std::fs::metadata(&path)?.len() <= 16_384,
        "Claude login completion is too large"
    );
    crate::storage::read_json(&path)
}

pub fn claim(path: &Path) -> Result<String> {
    let parent = path
        .parent()
        .context("Missing Claude login flow directory")?;
    read_receipt(path)?;
    let _lock = super::account_store::lock_store(parent, "result.json")?;
    let mut receipt = read_receipt(path)?;
    anyhow::ensure!(
        !receipt.claimed && !receipt.finished && receipt.completed.is_none(),
        "Claude login attempt is already finished"
    );
    uuid::Uuid::parse_str(&receipt.attempt).context("Invalid Claude login attempt")?;
    receipt.claimed = true;
    crate::storage::write_json_secret(path, &receipt)?;
    Ok(receipt.attempt)
}

pub fn finish(path: &Path, attempt: &str, completed: Option<CompletedLogin>) -> Result<()> {
    let parent = path
        .parent()
        .context("Missing Claude login flow directory")?;
    read_receipt(path)?;
    let _lock = super::account_store::lock_store(parent, "result.json")?;
    let receipt = read_receipt(path)?;
    anyhow::ensure!(
        receipt.claimed && !receipt.finished && receipt.attempt == attempt,
        "Claude login completion belongs to another attempt"
    );
    crate::storage::write_json_secret(
        path,
        &Receipt {
            attempt: attempt.into(),
            claimed: true,
            finished: true,
            completed,
        },
    )
}

pub fn fingerprint(credentials: &claude::ClaudeCredentials) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update((credentials.access_token.len() as u64).to_le_bytes());
    hash.update(credentials.access_token.as_bytes());
    hash.update(credentials.refresh_token.as_bytes());
    hex::encode(hash.finalize())
}

pub fn capture(
    credentials: &claude::ClaudeCredentials,
    source: LoginSource,
) -> Result<CompletedLogin> {
    let _lock = super::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    let auth = claude::load_auth_file()?;
    let mut matches = auth.anthropic_accounts.iter().filter(|account| {
        account.access == credentials.access_token && account.refresh == credentials.refresh_token
    });
    let saved_account = matches.next().map(|account| account.label.clone());
    anyhow::ensure!(
        matches.next().is_none(),
        "Claude login matches more than one saved account"
    );
    anyhow::ensure!(
        fingerprint(&claude::load_credentials()?) == fingerprint(credentials),
        "Claude login selection changed before completion"
    );
    Ok(CompletedLogin {
        source,
        fingerprint: fingerprint(credentials),
        saved_account,
        persisted_default: auth.active_anthropic_account,
    })
}

fn source_allowed(source: &LoginSource) -> Result<()> {
    let allowed = match source {
        LoginSource::Native => claude::native_source_allowed(),
        LoginSource::File(path) => {
            let source = claude::ExternalClaudeAuthSource::ClaudeCode;
            source.path()? == *path
                && crate::config::Config::external_auth_source_allowed_for_path(
                    source.source_id(),
                    path,
                )
        }
    };
    anyhow::ensure!(
        allowed,
        "Claude credential approval was revoked or its source changed during login"
    );
    Ok(())
}

/// Apply only the result this attempt produced, preserving a later local choice.
pub fn apply(completed: &CompletedLogin, expected_override: Option<&str>) -> Result<()> {
    let _lock = super::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    anyhow::ensure!(
        claude::get_active_account_override().as_deref() == expected_override,
        "Claude account selection changed while login was running"
    );
    validate(completed)?;
    let previous = claude::get_active_account_override();
    claude::set_active_account_override(completed.saved_account.clone());
    let result = (|| {
        source_allowed(&completed.source)?;
        anyhow::ensure!(
            fingerprint(&claude::load_credentials()?) == completed.fingerprint,
            "Another credential source takes precedence over the completed login"
        );
        Ok(())
    })();
    if result.is_err() {
        claude::set_active_account_override(previous);
    }
    result
}

fn validate(completed: &CompletedLogin) -> Result<()> {
    source_allowed(&completed.source)?;
    let auth = claude::load_auth_file()?;
    anyhow::ensure!(
        auth.active_anthropic_account == completed.persisted_default,
        "Saved Claude account selection changed after login completed"
    );
    if let Some(label) = &completed.saved_account {
        anyhow::ensure!(
            fingerprint(&claude::load_credentials_for_account(label)?) == completed.fingerprint,
            "Completed Claude login was replaced; retry sign-in"
        );
    }
    Ok(())
}

pub fn with_verified_receipt<T>(
    path: &Path,
    attempt: &str,
    fingerprint: &str,
    apply: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let parent = path
        .parent()
        .context("Missing Claude login flow directory")?;
    let _lock = super::account_store::lock_store(parent, "result.json")?;
    let receipt = read_receipt(path)?;
    anyhow::ensure!(
        receipt.finished && receipt.attempt == attempt,
        "Claude login was canceled or superseded"
    );
    let completed = receipt.completed.context("Claude login did not succeed")?;
    anyhow::ensure!(
        completed.fingerprint == fingerprint,
        "Claude login result changed"
    );
    validate(&completed)?;
    apply()
}

impl Drop for LoginFlow {
    fn drop(&mut self) {
        if let Err(error) = self.cancel() {
            crate::logging::debug(&format!("Could not retire Claude sign-in receipt: {error}"));
        }
    }
}
