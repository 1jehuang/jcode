use super::*;

/// `None` means no login is saved under `label` and an approved external
/// source still holds the token, so it may refresh.
/// A saved login removed while its refresh was pending is rejected instead.
pub(in crate::auth) fn capture_refresh_account(
    label: &str,
    refresh_token: &str,
) -> Result<Option<AnthropicAccount>> {
    let account = load_auth_file()?
        .anthropic_accounts
        .into_iter()
        .find(|account| account.label == label);
    anyhow::ensure!(
        !refresh_token.is_empty()
            && account.as_ref().map_or_else(
                || approved_external_source_holds(refresh_token),
                |account| account.refresh == refresh_token
            ),
        "Claude login changed before token refresh; retry with the current login"
    );
    Ok(account)
}

/// Same consent rules as `load_credentials`, uncached so revocation applies.
fn approved_external_source_holds(refresh_token: &str) -> bool {
    use crate::config::Config;
    let holds = |creds: super::ClaudeCredentials| creds.refresh_token == refresh_token;
    let claude_code = super::claude_code_path().is_ok_and(|path| {
        Config::external_auth_source_allowed_for_path(super::CLAUDE_CODE_AUTH_SOURCE_ID, &path)
    }) && super::load_claude_code_credentials().is_ok_and(holds);
    let native = super::native_source_allowed()
        && super::load_claude_code_env_credentials().is_some_and(holds);
    let opencode = super::opencode_path().is_ok_and(|path| {
        Config::external_auth_source_allowed_for_path(super::OPENCODE_AUTH_SOURCE_ID, &path)
            || Config::external_auth_source_allowed_for_path(
                crate::auth::external::OPENCODE_AUTH_JSON_SOURCE_ID,
                &path,
            )
    }) && super::load_opencode_credentials().is_ok_and(holds);
    let other = crate::auth::external::has_approved_anthropic_refresh_token(refresh_token);
    claude_code || native || opencode || other
}

/// Commit tokens refreshed after [`capture_refresh_account`], retaining the original token.
pub(in crate::auth) fn commit_captured_refresh(
    label: &str,
    expected: Option<&AnthropicAccount>,
    refresh_token: &str,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    match expected {
        Some(expected) => commit_refresh_account(expected, tokens),
        None => commit_external_refresh(label, refresh_token, tokens),
    }
}

/// Anthropic rotates refresh tokens, so an external login's old token is spent
/// once refreshed. Keep the new tokens, unless a login took the label meanwhile.
pub(super) fn commit_external_refresh(
    label: &str,
    refresh_token: &str,
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
    anyhow::ensure!(
        !refresh_token.is_empty() && approved_external_source_holds(refresh_token),
        "Claude external login approval changed while token refresh was pending; refreshed tokens were not saved"
    );
    let saved_label = crate::auth::account_store::upsert_account(
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
    save_auth_file(&auth)?;
    crate::usage::invalidate_anthropic_usage_auth_failure(&saved_label);
    Ok(())
}

pub(super) fn commit_refresh_account(
    expected: &AnthropicAccount,
    tokens: &crate::auth::oauth::OAuthTokens,
) -> Result<()> {
    let _store_lock =
        crate::auth::account_store::lock_store(&crate::storage::jcode_dir()?, "auth.json")?;
    let mut auth = load_auth_file()?;
    let mut matches = auth.anthropic_accounts.iter_mut().filter(|account| {
        let known_email = |account: &AnthropicAccount| {
            account
                .email
                .as_deref()
                .map(str::trim)
                .filter(|email| !email.is_empty())
                .map(str::to_ascii_lowercase)
        };
        let conflicting_email = known_email(account)
            .zip(known_email(expected))
            .is_some_and(|(current, original)| current != original);
        account.refresh == expected.refresh && !conflicting_email
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
    account.access = tokens.access_token.clone();
    account.refresh = tokens.refresh_token.clone();
    account.expires = tokens.expires_at;
    if !tokens.scopes.is_empty() {
        account.scopes = tokens.scopes.clone();
    }
    save_auth_file(&auth)?;
    crate::usage::invalidate_anthropic_usage_auth_failure(&label);
    Ok(())
}
