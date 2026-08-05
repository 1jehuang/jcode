//! CLI orchestration for the Jcode account device authorization flow.
//!
//! Protocol parsing and HTTP behavior live in `subscription_api` so the CLI and
//! TUI share the same contract and redaction guarantees.

use anyhow::{Context, Result};
use std::future::Future;
use std::time::Duration;

use crate::subscription_api::{
    self, ActivationOutcome, ApprovedAccountKey, PollingBackoff, TokenPollOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoginCompletion {
    Active,
    KeySavedPlanPending,
    CanceledBeforeApproval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum KeyPollCompletion {
    Approved(ApprovedAccountKey),
    Canceled,
}

pub(super) async fn poll_for_api_key<C>(
    client: &reqwest::Client,
    api_base: &str,
    device_code: &str,
    interval: u64,
    expires_in: u64,
    cancel: C,
) -> Result<KeyPollCompletion>
where
    C: Future<Output = std::io::Result<()>>,
{
    tokio::pin!(cancel);
    let base_delay = Duration::from_secs(interval.max(1));
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(expires_in.max(interval.max(1)));
    let mut backoff = PollingBackoff::new(base_delay);
    let mut reported_offline = false;

    loop {
        let delay = backoff.delay();
        if tokio::time::Instant::now() + delay >= deadline {
            anyhow::bail!(
                "Jcode account login timed out before browser approval. Run `jcode account login` to try again."
            );
        }
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            signal = &mut cancel => {
                signal.context("Failed to listen for Ctrl-C")?;
                return Ok(KeyPollCompletion::Canceled);
            }
        }

        // Deliberately do not poll cancellation while an exchange request is
        // in flight. The backend may atomically consume the one-time device
        // credential before the response reaches us. Finishing this bounded
        // request and persisting an approved key avoids stranding a live key
        // that the user can neither see nor revoke.
        match subscription_api::poll_device_token_once(client, api_base, device_code).await {
            Ok(TokenPollOutcome::Pending) => {
                backoff.on_pending();
                reported_offline = false;
            }
            Ok(TokenPollOutcome::SlowDown { retry_after }) => {
                backoff.on_slow_down(retry_after);
                reported_offline = false;
            }
            Ok(TokenPollOutcome::Approved(key)) => {
                return Ok(KeyPollCompletion::Approved(key));
            }
            Ok(TokenPollOutcome::Expired) => anyhow::bail!(
                "The browser approval expired or was already exchanged. Run `jcode account login` to start a new single-use flow."
            ),
            Ok(TokenPollOutcome::Denied) => {
                anyhow::bail!("Jcode account login was canceled or denied in the browser.")
            }
            Err(error) if error.is_temporary() => {
                if !reported_offline {
                    eprintln!("  Connection interrupted. Retrying with backoff...");
                    reported_offline = true;
                }
                backoff.on_offline_error();
            }
            Err(error) => return Err(anyhow::Error::new(error)),
        }
    }
}

fn persist_approved_key(approved: &ApprovedAccountKey) -> Result<()> {
    crate::subscription_catalog::persist_account_credentials(
        &approved.api_key,
        Some(&approved.account_id),
        Some(&approved.email),
        Some(&approved.tier),
    )?;
    crate::auth::AuthStatus::invalidate_cache();
    Ok(())
}

/// Full browser-first device login. No email or secret is requested in the
/// terminal. A valid exchanged key is retained when hosted billing activation
/// times out or the user cancels activation polling.
pub(super) async fn login_jcode_device_flow(no_browser: bool) -> Result<LoginCompletion> {
    let client = crate::provider::shared_http_client();
    let api_base = subscription_api::configured_api_base();
    let device = subscription_api::request_device_authorization(&client, &api_base, None)
        .await
        .map_err(anyhow::Error::new)
        .context("Failed to start Jcode account login")?;

    eprintln!("\nJcode Account Login");
    eprintln!("  Opening the secure account approval page:");
    eprintln!("  {}", device.verification_uri_complete);
    eprintln!("\n  Approve the request in that browser. No terminal email entry is needed.");
    super::maybe_open_browser(&device.verification_uri_complete, no_browser);
    eprintln!("  Waiting for browser approval. Press Ctrl-C to cancel...");

    let approved = match poll_for_api_key(
        &client,
        &api_base,
        &device.device_code,
        device.interval,
        device.expires_in,
        tokio::signal::ctrl_c(),
    )
    .await?
    {
        KeyPollCompletion::Approved(approved) => approved,
        KeyPollCompletion::Canceled => {
            eprintln!("\n  Login canceled before approval. No credential was saved.");
            return Ok(LoginCompletion::CanceledBeforeApproval);
        }
    };

    persist_approved_key(&approved)?;
    eprintln!("\n  Account approved for {}.", approved.email);
    eprintln!("  Credential saved securely with owner-only permissions.");
    eprintln!("  Waiting for hosted billing and your spending limit on /v1/me...");

    let activation = tokio::select! {
        result = subscription_api::poll_for_paid_activation(
            &client,
            &api_base,
            &approved.api_key,
            subscription_api::ACTIVATION_TIMEOUT,
            Duration::from_secs(device.interval.max(2)),
        ) => Some(result),
        signal = tokio::signal::ctrl_c() => {
            signal.context("Failed to listen for Ctrl-C")?;
            None
        }
    };

    let completion = match activation {
        Some(ActivationOutcome::Active(me)) => {
            crate::subscription_catalog::persist_account_credentials(
                &approved.api_key,
                Some(&me.account_id),
                Some(&me.email),
                Some(&me.tier),
            )?;
            eprintln!(
                "  ✓ Hosted models are ready with a ${:.2} monthly spending limit.",
                me.usage.budget_usd
            );
            LoginCompletion::Active
        }
        Some(ActivationOutcome::Canceled(_)) => {
            eprintln!(
                "  Billing setup was canceled. Your account key remains saved, but hosted usage is not enabled."
            );
            print_recovery_actions();
            LoginCompletion::KeySavedPlanPending
        }
        Some(ActivationOutcome::TimedOut {
            last_error_was_offline,
        }) => {
            if last_error_was_offline {
                eprintln!(
                    "  Hosted billing could not be confirmed before timeout because the account API remained unreachable."
                );
            } else {
                eprintln!("  A spending limit was not detected before timeout.");
            }
            eprintln!("  Your valid account key remains saved.");
            print_recovery_actions();
            LoginCompletion::KeySavedPlanPending
        }
        Some(ActivationOutcome::Revoked) => {
            crate::subscription_catalog::clear_account_credentials()?;
            anyhow::bail!(
                "The newly issued account key was revoked before hosted billing activation. Local credentials were cleared; run `jcode account login` again."
            );
        }
        Some(ActivationOutcome::Denied) => {
            crate::subscription_catalog::clear_account_credentials()?;
            anyhow::bail!(
                "The account server denied hosted billing checks. Local credentials were cleared; run `jcode account login` again."
            );
        }
        None => {
            eprintln!("\n  Activation wait canceled. Your valid account key remains saved.");
            print_recovery_actions();
            LoginCompletion::KeySavedPlanPending
        }
    };

    crate::telemetry::record_auth_success("jcode-subscription", "device_code_browser");
    Ok(completion)
}

fn print_recovery_actions() {
    eprintln!("  Check:   jcode account status");
    eprintln!("  Manage:  jcode account manage");
    eprintln!("  Log out: jcode account logout");
}

#[cfg(test)]
mod tests;
