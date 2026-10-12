use super::{Path, Result, auth};

pub(super) fn reuse_claude_code_native_snapshot(
    file_path: &Path,
    credentials: auth::claude::ClaudeCredentials,
    before_validation: impl FnOnce(),
) -> Result<bool> {
    let file_source = auth::claude::ExternalClaudeAuthSource::ClaudeCode;
    if crate::config::Config::external_auth_source_allowed_for_path(
        file_source.source_id(),
        file_path,
    ) && auth::claude::load_claude_code_credentials().is_ok_and(|file| {
        file.expires_at > chrono::Utc::now().timestamp_millis()
            || credentials.expires_at <= chrono::Utc::now().timestamp_millis()
    }) {
        anyhow::bail!(
            "A previously trusted Claude Code credentials file takes precedence over the native login. Revoke that file's trust in Jcode's external auth settings, or use its existing login instead. No trust was changed."
        );
    }

    // Validate precedence using an in-memory candidate. A rejected attempt must
    // not import an account, activate it, or change persistent native trust.
    before_validation();
    let selected = auth::claude::load_credentials_with_staged_native(&credentials)?;
    anyhow::ensure!(
        selected.access_token == credentials.access_token
            && selected.refresh_token == credentials.refresh_token,
        "Another trusted Claude credential source is taking precedence over this login. Review Jcode's trusted external auth sources before retrying."
    );

    if credentials.refresh_token.trim().is_empty() {
        anyhow::ensure!(
            std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
                .ok()
                .is_some_and(|token| !token.trim().is_empty()),
            "Claude Code's Keychain credential has no refresh token and cannot be imported."
        );
        eprintln!(
            "Using the Claude Code environment token without copying it; it must remain configured for future sessions."
        );
    }
    if !auth::claude::native_source_allowed() {
        auth::claude::trust_native_source()?;
    }
    if !credentials.refresh_token.trim().is_empty() {
        let label = auth::claude::import_native_credentials_into_account_from(credentials)?;
        auth::claude::set_active_account(&label)?;
    }
    Ok(true)
}
