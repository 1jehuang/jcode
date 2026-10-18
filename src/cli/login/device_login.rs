//! Interactive device-flow logins (GitHub Copilot, native Muse subscription).
//!
//! These flows show a verification URL + user code, optionally open a browser,
//! then poll until the user approves. The Muse subscription flow is
//! experimental and gated by `auth::muse::ensure_enabled()`; its poll loop is
//! shared with the scriptable `--complete` path.

use super::{LoginFlowOutcome, maybe_open_browser};
use crate::auth;
use anyhow::Result;

pub(super) fn login_copilot_flow(no_browser: bool) -> Result<LoginFlowOutcome> {
    eprintln!("Starting GitHub Copilot login...");

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(login_copilot_device_flow(no_browser))
    })?;
    Ok(LoginFlowOutcome::Completed)
}

async fn login_copilot_device_flow(no_browser: bool) -> Result<()> {
    let client = crate::provider::shared_http_client();

    let device_resp = crate::auth::copilot::initiate_device_flow(&client).await?;

    eprintln!();
    eprintln!("  Open this URL in your browser:");
    eprintln!("    {}", device_resp.verification_uri);
    eprintln!();
    if let Some(qr) = jcode_terminal_prompt::qr::indented_section(
        &device_resp.verification_uri,
        "  Or scan this QR on another device to open the verification page:",
        "    ",
        crate::auth::browser_suppressed(no_browser),
    ) {
        eprintln!("{qr}");
        eprintln!();
    }
    eprintln!("  Enter code: {}", device_resp.user_code);
    eprintln!();
    eprintln!("  Waiting for authorization...");

    maybe_open_browser(&device_resp.verification_uri, no_browser);

    let token = crate::auth::copilot::poll_for_access_token(
        &client,
        &device_resp.device_code,
        device_resp.interval,
    )
    .await?;

    let username = crate::auth::copilot::fetch_github_username(&client, &token)
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    crate::auth::copilot::save_github_token(&token, &username)?;

    eprintln!("  ✓ Authenticated as {} via GitHub Copilot", username);
    crate::telemetry::record_auth_success("copilot", "oauth_device_code");
    Ok(())
}

pub(super) async fn login_muse_subscription_flow(no_browser: bool) -> Result<LoginFlowOutcome> {
    // Native Muse subscription (experimental, explicit opt-in). The gate lives
    // in auth::muse so every surface (CLI, scriptable, TUI) enforces the same
    // JCODE_EXPERIMENTAL_MUSE_SUBSCRIPTION=1 requirement.
    auth::muse::ensure_enabled()?;
    eprintln!("{}", auth::muse::experimental_warning_message());
    eprintln!("Starting native Muse subscription login...");
    eprintln!("No local Muse installation is required.\n");

    let client = crate::auth::muse::http_client()?;
    let challenge = auth::muse::begin_login(&client).await?;
    let url = challenge
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&challenge.verification_uri);
    eprintln!("  Open: {url}");
    eprintln!("  Code: {}\n", challenge.user_code);
    maybe_open_browser(url, no_browser);
    eprintln!("Waiting for authorization...");

    let credentials = poll_muse_subscription_until_decided(&client, &challenge).await?;
    auth::muse::save_credentials(&credentials)?;
    eprintln!("Muse subscription login complete.");
    eprintln!(
        "Credentials saved to {}",
        auth::muse::credentials_path()?.display()
    );
    crate::telemetry::record_auth_success("muse-subscription", "oauth_device_code");
    Ok(LoginFlowOutcome::Completed)
}

/// Drive shark's single-attempt `auth::muse::poll_login` to a terminal outcome.
///
/// Pending/SlowDown sleep the caller-owned backoff here so both the interactive
/// CLI flow and the scriptable `--complete` path share the same loop.
pub(super) async fn poll_muse_subscription_until_decided(
    client: &reqwest::Client,
    challenge: &auth::muse::DeviceChallenge,
) -> Result<auth::muse::MuseCredentials> {
    let interval_secs = u64::try_from(challenge.interval).unwrap_or(5).max(1);
    let expires_secs = u64::try_from(challenge.expires_in).unwrap_or(1800).max(60);
    let mut wait = std::time::Duration::from_secs(interval_secs);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_secs);
    loop {
        match auth::muse::poll_login(client, challenge).await? {
            auth::muse::PollOutcome::Approved(credentials) => return Ok(credentials),
            auth::muse::PollOutcome::Denied => {
                anyhow::bail!("Muse subscription login was denied or cancelled in the browser.")
            }
            auth::muse::PollOutcome::Expired => {
                anyhow::bail!("The Muse device code expired. Start a new login.")
            }
            auth::muse::PollOutcome::Pending => {}
            auth::muse::PollOutcome::SlowDown { retry_after } => {
                wait = std::time::Duration::from_secs(retry_after.max(wait.as_secs()));
            }
        }
        if std::time::Instant::now() + wait >= deadline {
            anyhow::bail!("Timed out waiting for Muse browser authorization.");
        }
        tokio::time::sleep(wait).await;
    }
}
