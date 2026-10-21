use super::{AnthropicAccount, JcodeAuthFile, account_identity, load_auth_file, save_auth_file};
use anyhow::Result;

/// Replace exactly one existing saved login, without adding a label or moving defaults.
pub fn replace_account_if_unchanged(
    label: &str,
    expected_identity: Option<&str>,
    replacement: AnthropicAccount,
) -> Result<()> {
    let mut auth = load_auth_file()?;
    replace_saved_login(&mut auth, label, expected_identity, replacement)?;
    save_auth_file(&auth)
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
