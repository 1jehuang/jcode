//! Refresh commits share a stable sidecar lock with every Codex store writer.
//! Never lock the JSON inode: atomic replacement changes that inode.
use super::*;
use std::fs::{File, OpenOptions};

pub(super) fn lock_auth_file() -> Result<File> {
    let path = jcode_auth_path()?.with_extension("lock");
    std::fs::create_dir_all(path.parent().context("OpenAI auth lock has no directory")?)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        anyhow::bail!("OpenAI auth lock must not be a symlink");
    }
    let file = options
        .open(&path)
        .context("Could not open OpenAI auth lock")?;
    file.lock().context("Could not lock OpenAI auth store")?;
    Ok(file)
}

// Deliberately not Debug: this snapshot contains the original refresh secret.
pub(in crate::auth) struct RefreshAccount {
    refresh_token: String,
    identity: Option<String>,
}

pub(in crate::auth) fn capture_refresh_account(
    label: &str,
    refresh_token: &str,
) -> Result<RefreshAccount> {
    let auth = load_auth_file()?;
    let account = auth
        .openai_accounts
        .iter()
        .find(|a| a.label == label)
        .filter(|a| !refresh_token.is_empty() && a.refresh_token == refresh_token)
        .context("OpenAI login changed before token refresh; retry with the current login")?;
    Ok(RefreshAccount {
        refresh_token: account.refresh_token.clone(),
        identity: account_identity(account),
    })
}

pub(in crate::auth) fn commit_refresh_account(
    expected: &RefreshAccount,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    let _lock = lock_auth_file()?;
    let mut auth = load_auth_file_unlocked()?;
    apply_refresh(&mut auth, expected, tokens)?;
    save_auth_file_unlocked(&auth)
}

fn apply_refresh(
    auth: &mut JcodeOpenAiAuthFile,
    expected: &RefreshAccount,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    // A label is presentation, not identity. Follow a rename only when exactly
    // one record still has BOTH the original login identity and refresh token.
    let mut matches = auth
        .openai_accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| {
            account.refresh_token == expected.refresh_token
                && account_identity(account) == expected.identity
        });
    let index = matches.next().map(|(index, _)| index).context(
        "OpenAI login changed while token refresh was pending; refreshed tokens were not saved",
    )?;
    if matches.next().is_some() {
        anyhow::bail!("OpenAI refresh login is ambiguous; refreshed tokens were not saved");
    }
    let account = &mut auth.openai_accounts[index];
    // Do not attach claims from a different workspace to the original login.
    let original_account_id = account
        .account_id
        .clone()
        .or_else(|| account.id_token.as_deref().and_then(extract_account_id));
    if let Some(id) = tokens.id_token.as_deref().and_then(extract_account_id)
        && original_account_id.as_deref().is_some_and(|old| old != id)
    {
        anyhow::bail!(
            "OpenAI refresh returned a different account identity; tokens were not saved"
        );
    }
    account.access_token = tokens.access_token.clone();
    account.refresh_token = tokens.refresh_token.clone();
    account.expires_at = Some(tokens.expires_at);
    if let Some(id_token) = tokens.id_token.as_ref() {
        account.account_id = extract_account_id(id_token).or_else(|| account.account_id.clone());
        account.email = extract_email(id_token).or_else(|| account.email.clone());
        account.id_token = Some(id_token.clone());
    }
    Ok(())
}

#[cfg(test)]
#[path = "codex_refresh_cas_tests.rs"]
mod tests;
