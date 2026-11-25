use super::*;
#[path = "native_reuse.rs"]
mod native_reuse;
use native_reuse::reuse_claude_code_native_snapshot;

pub(super) async fn login_claude_flow(
    requested_label: Option<&str>,
    no_browser: bool,
) -> Result<()> {
    let label = auth::claude::login_target_label(requested_label)?;
    eprintln!("Logging in to Claude (account: {})...", label);
    let tokens = auth::oauth::login_claude(no_browser).await?;
    auth::oauth::save_claude_tokens_for_account(&tokens, &label)?;
    let profile_email =
        match auth::oauth::update_claude_account_profile(&label, &tokens.access_token).await {
            Ok(email) => email,
            Err(e) => {
                eprintln!(
                    "Warning: logged in but failed to fetch profile metadata: {}",
                    e
                );
                None
            }
        };
    eprintln!("Successfully logged in to Claude!");
    eprintln!(
        "Account '{}' stored at {}",
        label,
        auth::claude::jcode_path()?.display()
    );
    if let Some(email) = profile_email {
        eprintln!("Profile email: {}", email);
    }
    crate::telemetry::record_auth_success("claude", "oauth");
    Ok(())
}

pub(super) fn validate_claude_code_method(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
    options: &LoginOptions,
) -> Result<()> {
    if !options.claude_code {
        return Ok(());
    }
    anyhow::ensure!(
        provider.target == LoginProviderTarget::Claude,
        "--claude-code is only available with --provider claude."
    );
    anyhow::ensure!(
        account_label.is_none(),
        "--claude-code cannot select a Jcode account label; Claude Code chooses its own account."
    );
    anyhow::ensure!(
        !options.no_browser && !options.json && !options.uses_scriptable_flow()?,
        "--claude-code requires an interactive terminal and cannot use scriptable or --no-browser login flags."
    );
    anyhow::ensure!(
        crate::external_auth::can_prompt_for_external_auth(),
        "--claude-code requires an interactive terminal on this machine. For SSH, run the command on the remote host."
    );
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Test-only answer for the Claude Code reuse prompt.
    static CLAUDE_CODE_REUSE_TEST_APPROVAL: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

pub(super) fn confirm_claude_code_reuse(native: bool) -> Result<()> {
    #[cfg(test)]
    if let Some(approved) = CLAUDE_CODE_REUSE_TEST_APPROVAL.with(std::cell::Cell::get) {
        anyhow::ensure!(
            approved,
            "Claude Code signed in, but Jcode was not granted access to its credentials."
        );
        return Ok(());
    }
    eprintln!(
        "\nClaude Code signed in. Jcode makes direct Anthropic requests, so it needs permission to reuse the CLI's OAuth credentials."
    );
    if native {
        eprintln!(
            "Jcode copies macOS Keychain credentials into its own store. Both tools may refresh the same token, so either login may need renewal."
        );
    } else {
        eprintln!("Jcode will read Claude Code's credentials file for future requests.");
    }
    eprint!("Allow Jcode to use this Claude Code login? [y/N]: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(
        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        "Claude Code signed in, but Jcode was not granted access to its credentials."
    );
    Ok(())
}

pub(super) fn usable_claude_code_credentials(
    credentials: &auth::claude::ClaudeCredentials,
) -> bool {
    !credentials.refresh_token.trim().is_empty()
        || (!credentials.access_token.trim().is_empty()
            && credentials.expires_at > chrono::Utc::now().timestamp_millis())
}

pub(super) fn ensure_claude_code_credentials_selected(
    expected: &auth::claude::ClaudeCredentials,
) -> Result<()> {
    let selected = auth::claude::load_credentials()?;
    anyhow::ensure!(
        selected.access_token == expected.access_token
            && selected.refresh_token == expected.refresh_token,
        "Another trusted Claude credential source is taking precedence over this login. Review Jcode's trusted external auth sources before retrying."
    );
    Ok(())
}

pub(super) fn reuse_claude_code_file(
    source: auth::claude::ExternalClaudeAuthSource,
    path: &Path,
) -> Result<Option<auth::claude::ClaudeCredentials>> {
    let already_trusted =
        crate::config::Config::external_auth_source_allowed_for_path(source.source_id(), path);
    if !already_trusted {
        confirm_claude_code_reuse(false)?;
    }
    let credentials = match auth::claude::load_claude_code_credentials() {
        Ok(credentials) if usable_claude_code_credentials(&credentials) => credentials,
        _ => return Ok(None),
    };
    if !already_trusted {
        auth::claude::trust_external_auth_source(source)?;
    }
    if let Err(err) = ensure_claude_code_credentials_selected(&credentials) {
        // Do not leave a file trusted by a login that did not take effect: a
        // later update could otherwise make Jcode select it without approval.
        if !already_trusted {
            crate::config::Config::revoke_external_auth_source_for_path(source.source_id(), path)?;
            auth::AuthStatus::invalidate_cache();
        }
        return Err(err);
    }
    Ok(Some(credentials))
}

pub(super) fn reuse_claude_code_native(
    file_path: &Path,
) -> Result<Option<auth::claude::ClaudeCredentials>> {
    let expected_selection = auth::claude::native_import_selection()?;
    if !auth::claude::native_credentials_present() {
        return Ok(None);
    }
    let was_native_trusted = auth::claude::native_source_allowed();
    if !was_native_trusted {
        confirm_claude_code_reuse(true)?;
    }
    // Reading Keychain secrets is permitted only after approval. The item may
    // exist but be locked, in which case the file source can still be tried.
    let credentials = match auth::claude::load_native_credentials() {
        Ok(credentials) if usable_claude_code_credentials(&credentials) => credentials,
        _ => return Ok(None),
    };

    reuse_claude_code_native_snapshot(
        file_path,
        was_native_trusted,
        &expected_selection,
        credentials.clone(),
        || {},
    )?;
    Ok(Some(credentials))
}

pub(super) fn configured_claude_file_is_unambiguous(
    path: &Path,
    existed_before_login: bool,
    modified_before_login: Option<std::time::SystemTime>,
    native_present: bool,
) -> Result<()> {
    if existed_before_login && native_present {
        let modified_after_login = credential_file_modified(path)?;
        anyhow::ensure!(
            modified_before_login
                .zip(modified_after_login)
                .is_some_and(|(before, after)| before != after),
            "Claude Code did not update the configured credentials file, while native credentials also exist. Jcode cannot tell which account just signed in. Refresh or remove the stale file, or use Jcode's OAuth login."
        );
    }
    Ok(())
}

pub(super) fn login_claude_code_flow() -> Result<auth::claude_cli_completion::CompletedLogin> {
    let expected_selection = auth::claude::native_import_selection()?;
    let source = auth::claude::ExternalClaudeAuthSource::ClaudeCode;
    let file_path = source.path()?;
    let configured_file = std::env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|dir| !dir.is_empty());
    let file_modified_before_login = credential_file_modified(&file_path)?;
    let file_existed_before_login = file_modified_before_login.is_some();
    eprintln!("Signing in with Claude Code on this machine (`claude auth login`)…");
    let status = std::process::Command::new("claude")
        .args(["auth", "login"])
        .status()
        .context("Failed to start command: claude auth login")?;
    anyhow::ensure!(
        status.success(),
        "Command exited with non-zero status: claude auth login ({status})"
    );

    anyhow::ensure!(
        auth::claude::native_import_selection()? == expected_selection,
        "The saved Claude account selection changed while Claude Code was signing in. Retry to use that login."
    );

    // Jcode makes direct Anthropic requests, so a CLI login by itself is not a
    // Jcode login. Keep the existing external-source consent boundary intact.
    let file_exists_after_login = credential_file_modified(&file_path)?.is_some();
    let mut reused = None;
    let mut native = false;
    if configured_file && file_exists_after_login {
        configured_claude_file_is_unambiguous(
            &file_path,
            file_existed_before_login,
            file_modified_before_login,
            auth::claude::native_credentials_present(),
        )?;
        reused = reuse_claude_code_file(source, &file_path)?;
    }
    if reused.is_none() {
        reused = reuse_claude_code_native(&file_path)?;
        native = reused.is_some();
    }
    if reused.is_none() && !configured_file && file_exists_after_login {
        reused = reuse_claude_code_file(source, &file_path)?;
    }
    anyhow::ensure!(
        reused.is_some(),
        "Claude Code signed in, but no reusable OAuth credentials were found. Check `claude auth status`, or use `jcode login --provider claude` for Jcode's OAuth flow."
    );
    let credentials = reused.context("Claude Code did not produce a completed login")?;
    let source = if native {
        auth::claude_cli_completion::LoginSource::Native
    } else {
        auth::claude_cli_completion::LoginSource::File(file_path)
    };
    let completed = auth::claude_cli_completion::capture(&credentials, source)?;
    eprintln!("Claude Code login is now available to Jcode.");
    crate::telemetry::record_auth_success("claude", "claude_code_cli");
    Ok(completed)
}

#[cfg(test)]
#[path = "native_stage_tests.rs"]
mod native_stage_tests;
#[cfg(test)]
#[path = "claude_cli_tests.rs"]
mod tests;

fn credential_file_modified(path: &Path) -> Result<Option<std::time::SystemTime>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Claude credential path must be a regular file"
            );
            Ok(Some(metadata.modified().context(
                "Could not read Claude credential file timestamp",
            )?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Could not inspect Claude credential file"),
    }
}
