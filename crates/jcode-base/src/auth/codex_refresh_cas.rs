//! Refresh commits share a stable sidecar lock with every Codex store writer.
//! Never lock the JSON inode: atomic replacement changes that inode.
use super::*;
pub(super) fn lock_auth_file() -> Result<crate::auth::account_store::StoreLockGuard> {
    crate::auth::account_store::lock_store(&crate::storage::jcode_dir()?, "openai-auth.json")
}

// Deliberately not Debug: this snapshot contains the original refresh secret.
pub(in crate::auth) struct RefreshAccount {
    refresh_token: String,
    workspace: Option<String>,
    email: Option<String>,
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
    let (workspace, email) = identity_claims(account);
    Ok(RefreshAccount {
        refresh_token: account.refresh_token.clone(),
        workspace,
        email,
    })
}

pub(in crate::auth) fn commit_refresh_account(
    expected: &RefreshAccount,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    let _lock = lock_auth_file()?;
    let mut auth = load_auth_file()?;
    apply_refresh(&mut auth, expected, tokens)?;
    save_auth_file(&auth)
}

fn apply_refresh(
    auth: &mut JcodeOpenAiAuthFile,
    expected: &RefreshAccount,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    // A label is presentation, not identity. Follow a rename only when exactly
    // one record still has the original refresh token and no conflicting identity.
    let mut matches = auth
        .openai_accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| {
            account.refresh_token == expected.refresh_token && identity_matches(expected, account)
        });
    let index = matches.next().map(|(index, _)| index).context(
        "OpenAI login changed while token refresh was pending; refreshed tokens were not saved",
    )?;
    if matches.next().is_some() {
        anyhow::bail!("OpenAI refresh login is ambiguous; refreshed tokens were not saved");
    }
    let account = &mut auth.openai_accounts[index];
    // Do not attach claims from a different workspace to the original login.
    let (original_account_id, _) = identity_claims(account);
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

fn identity_claims(account: &OpenAiAccount) -> (Option<String>, Option<String>) {
    let nonempty = |value: Option<String>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let workspace = nonempty(account.account_id.clone())
        .or_else(|| nonempty(account.id_token.as_deref().and_then(extract_account_id)));
    let email = nonempty(account.email.clone())
        .or_else(|| nonempty(account.id_token.as_deref().and_then(extract_email)))
        .map(|email| email.to_ascii_lowercase());
    (workspace, email)
}

fn identity_matches(expected: &RefreshAccount, account: &OpenAiAccount) -> bool {
    let (workspace, email) = identity_claims(account);
    // The exact refresh generation already matches. New profile details are
    // enrichment; known workspace IDs remain authoritative over mutable email.
    match (&expected.workspace, workspace) {
        (Some(expected), Some(current)) => expected == &current,
        _ => match (&expected.email, email) {
            (Some(expected), Some(current)) => expected == &current,
            _ => true,
        },
    }
}

#[cfg(test)]
#[path = "codex_refresh_cas_tests.rs"]
mod tests;
