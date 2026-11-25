use super::{claude, codex};
use anyhow::{Context, Result};
use jcode_provider_core::AccountPin;

pub fn claude_identity(account: &claude::AnthropicAccount) -> Option<String> {
    super::account_store::account_identity(account.email.as_deref(), None, &account.refresh)
}

pub fn openai_identity(account: &codex::OpenAiAccount) -> Option<String> {
    let account_id = account.account_id.clone().or_else(|| {
        account
            .id_token
            .as_deref()
            .and_then(codex::extract_account_id)
    });
    let email = account
        .email
        .clone()
        .or_else(|| account.id_token.as_deref().and_then(codex::extract_email));
    super::account_store::account_identity(None, account_id.as_deref(), "").or_else(|| {
        super::account_store::account_identity(email.as_deref(), None, &account.refresh_token)
    })
}

pub fn claude_pin(label: &str) -> Result<AccountPin> {
    let accounts = claude::list_accounts()?;
    let account = accounts
        .iter()
        .find(|account| account.label == label)
        .context("Saved Claude account is unavailable")?;
    Ok(AccountPin {
        label: label.into(),
        identity: claude_identity(account),
        record_id: account.record_id.clone(),
    })
}

pub fn openai_pin(label: &str) -> Result<AccountPin> {
    let accounts = codex::list_accounts()?;
    let account = accounts
        .iter()
        .find(|account| account.label == label)
        .context("Saved OpenAI account is unavailable")?;
    Ok(AccountPin {
        label: label.into(),
        identity: openai_identity(account),
        record_id: account.record_id.clone(),
    })
}

pub fn resolve_claude(pin: &AccountPin) -> Result<String> {
    super::account_store::resolve_pin(
        pin,
        &claude::list_accounts()?,
        |account| &account.label,
        claude_identity,
        |account| account.record_id.as_deref(),
    )
    .context("Pinned Claude identity is missing or ambiguous; select the account again")
}

pub fn resolve_openai(pin: &AccountPin) -> Result<String> {
    let accounts = codex::list_accounts()?;
    if let Some(record_id) = &pin.record_id
        && !accounts
            .iter()
            .any(|account| account.record_id.as_ref() == Some(record_id))
        && !pin
            .identity
            .as_deref()
            .is_some_and(|identity| identity.starts_with("id:"))
    {
        anyhow::bail!("Pinned OpenAI record was removed; select the workspace again");
    }
    // Legacy email pins must identify exactly one workspace, even at the old label.
    if pin.record_id.is_none()
        && let Some(email) = pin.identity.as_deref().filter(|identity| {
            identity.contains('@') && !identity.starts_with("id:") && !identity.starts_with("rt:")
        })
    {
        let mut matches = accounts.iter().filter(|account| {
            account
                .email
                .clone()
                .or_else(|| account.id_token.as_deref().and_then(codex::extract_email))
                .is_some_and(|value| value.trim().eq_ignore_ascii_case(email))
        });
        let account = matches
            .next()
            .context("Pinned OpenAI email is unavailable")?;
        anyhow::ensure!(
            matches.next().is_none(),
            "Pinned OpenAI email is ambiguous; select the workspace again"
        );
        return Ok(account.label.clone());
    }
    super::account_store::resolve_pin(
        pin,
        &accounts,
        |account| &account.label,
        openai_identity,
        |account| account.record_id.as_deref(),
    )
    .context("Pinned OpenAI identity is missing or ambiguous; select the workspace again")
}

pub fn normalize_claude_pin(pin: &AccountPin) -> Result<AccountPin> {
    let _lock = super::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    claude_pin(&resolve_claude(pin)?)
}

pub fn normalize_openai_pin(pin: &AccountPin) -> Result<AccountPin> {
    let _lock =
        super::account_store::lock_store(&crate::storage::jcode_dir()?, "openai-auth.json")?;
    openai_pin(&resolve_openai(pin)?)
}

pub fn load_claude(pin: &AccountPin) -> Result<(claude::ClaudeCredentials, String)> {
    let _lock = super::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    let label = resolve_claude(pin)?;
    Ok((claude::load_credentials_for_account(&label)?, label))
}

pub fn load_openai(pin: &AccountPin) -> Result<(codex::CodexCredentials, String)> {
    let _lock =
        super::account_store::lock_store(&crate::storage::jcode_dir()?, "openai-auth.json")?;
    let label = resolve_openai(pin)?;
    Ok((codex::load_credentials_for_account(&label)?, label))
}

/// Export the account named by a strong pin while keeping resolution and export atomic.
pub fn export_cloud_pin(
    provider: super::transfer::TransferProvider,
    pin: &AccountPin,
) -> Result<super::transfer::CloudCredentialTransfer> {
    use super::transfer::{TransferProvider, export_cloud_account_at};
    let home = crate::storage::jcode_dir()?;
    let _lock = super::account_store::lock_store(&home, provider.store_name())?;
    let current = match provider {
        TransferProvider::Claude => claude_pin(&resolve_claude(pin)?)?,
        TransferProvider::OpenAi => openai_pin(&resolve_openai(pin)?)?,
    };
    anyhow::ensure!(
        pin.record_id.is_none() || pin.record_id == current.record_id,
        "Pinned cloud account was replaced; retry the handoff"
    );
    Ok(export_cloud_account_at(
        &home,
        provider,
        Some(&current.label),
    )?)
}

/// Rebind a foreign pin only to the credential proven by the private handoff.
pub fn rebind_cloud_pin(
    provider: super::transfer::TransferProvider,
    label: &str,
    expected_fingerprint: &str,
) -> Result<AccountPin> {
    use super::transfer::{TransferProvider, export_cloud_account_at};
    use sha2::{Digest, Sha256};
    let home = crate::storage::jcode_dir()?;
    let _lock = super::account_store::lock_store(&home, provider.store_name())?;
    let payload = export_cloud_account_at(&home, provider, Some(label))?;
    anyhow::ensure!(
        format!("{:x}", Sha256::digest(payload.as_bytes())) == expected_fingerprint,
        "Transferred OAuth credential changed; retry the handoff before starting the session"
    );
    match provider {
        TransferProvider::Claude => claude_pin(label),
        TransferProvider::OpenAi => openai_pin(label),
    }
}

#[cfg(test)]
#[path = "account_pin_identity_tests.rs"]
pub(crate) mod tests;
