use super::{AnthropicAccount, JcodeAuthFile, account_identity, load_auth_file, save_auth_file};
use anyhow::Result;

/// Replace exactly one existing saved login, without adding a label or moving defaults.
pub fn replace_account_if_unchanged(
    label: &str,
    expected_identity: Option<&str>,
    replacement: AnthropicAccount,
) -> Result<()> {
    let _store_lock =
        crate::auth::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    let mut auth = load_auth_file()?;
    replace_saved_login(&mut auth, label, expected_identity, replacement)?;
    save_auth_file(&auth)
}

/// `None` means no login is saved under `label`: the token came from an
/// approved external source (Claude Code, OpenCode) and may still refresh.
pub(in crate::auth) fn capture_refresh_account(
    label: &str,
    refresh_token: &str,
) -> Result<Option<AnthropicAccount>> {
    let Some(account) = load_auth_file()?
        .anthropic_accounts
        .into_iter()
        .find(|account| account.label == label)
    else {
        return Ok(None);
    };
    anyhow::ensure!(
        !refresh_token.is_empty() && account.refresh == refresh_token,
        "Claude login changed before token refresh; retry with the current login"
    );
    Ok(Some(account))
}

/// Commit tokens refreshed after [`capture_refresh_account`].
pub(in crate::auth) fn commit_captured_refresh(
    label: &str,
    expected: Option<&AnthropicAccount>,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    match expected {
        Some(expected) => commit_refresh_account(expected, tokens),
        None => commit_external_refresh(label, tokens),
    }
}

/// Anthropic rotates refresh tokens, so an external login's old token is spent
/// once refreshed. Keep the new tokens, unless a login took the label meanwhile.
pub(super) fn commit_external_refresh(
    label: &str,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    let _store_lock =
        crate::auth::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    let mut auth = load_auth_file()?;
    anyhow::ensure!(
        !auth
            .anthropic_accounts
            .iter()
            .any(|account| account.label == label),
        "Claude login changed while token refresh was pending; refreshed tokens were not saved"
    );
    crate::auth::account_store::upsert_account(
        super::ACCOUNT_LABEL_PREFIX,
        &mut auth.anthropic_accounts,
        &mut auth.active_anthropic_account,
        AnthropicAccount {
            label: label.to_string(),
            access: tokens.access_token.clone(),
            refresh: tokens.refresh_token.clone(),
            expires: tokens.expires_at,
            email: None,
            subscription_type: None,
            scopes: tokens.scopes.clone(),
        },
        |account| account.label.as_str(),
        |account, label| account.label = label,
    );
    save_auth_file(&auth)
}

pub(super) fn commit_refresh_account(
    expected: &AnthropicAccount,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    let _store_lock =
        crate::auth::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    let mut auth = load_auth_file()?;
    let mut matches = auth.anthropic_accounts.iter_mut().filter(|account| {
        account.refresh == expected.refresh
            && account_identity(account) == account_identity(expected)
    });
    let account = matches.next().ok_or_else(|| {
        anyhow::anyhow!(
            "Claude login changed while token refresh was pending; refreshed tokens were not saved"
        )
    })?;
    anyhow::ensure!(
        matches.next().is_none(),
        "Claude refresh login is ambiguous; refreshed tokens were not saved"
    );
    let label = account.label.clone();
    let previous_access = account.access.clone();
    account.access = tokens.access_token.clone();
    account.refresh = tokens.refresh_token.clone();
    account.expires = tokens.expires_at;
    if !tokens.scopes.is_empty() {
        account.scopes = tokens.scopes.clone();
    }
    save_auth_file(&auth)?;
    crate::usage::carry_anthropic_usage_across_token_refresh(
        &label,
        &previous_access,
        &tokens.access_token,
    );
    Ok(())
}

pub(super) fn replace_saved_login(
    auth: &mut JcodeAuthFile,
    label: &str,
    expected_identity: Option<&str>,
    mut replacement: AnthropicAccount,
) -> Result<()> {
    let index = auth
        .anthropic_accounts
        .iter()
        .position(|a| a.label == label)
        .ok_or_else(|| {
            anyhow::anyhow!("The saved login was removed or renamed. No credentials were replaced.")
        })?;
    if account_identity(&auth.anthropic_accounts[index]).as_deref() != expected_identity {
        anyhow::bail!(
            "The saved login changed while authentication was pending. No credentials were replaced."
        );
    }
    let email = replacement
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Could not verify the replacement login identity. No credentials were replaced."
            )
        })?;
    if auth.anthropic_accounts.iter().enumerate().any(|(i, a)| {
        i != index
            && a.email
                .as_deref()
                .is_some_and(|existing| existing.trim().eq_ignore_ascii_case(email))
    }) {
        anyhow::bail!(
            "This identity is already saved under another account. Use that saved login instead. No credentials were replaced."
        );
    }
    replacement.label = label.to_string();
    auth.anthropic_accounts[index] = replacement;
    Ok(())
}
