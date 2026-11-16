use anyhow::{Context, Result};

use crate::subscription_api::{self, AccountApiError};

pub(crate) async fn run_login(no_browser: bool) -> Result<()> {
    super::login::run_jcode_account_login(no_browser).await
}

pub(crate) async fn run_status(json: bool) -> Result<()> {
    let Some(api_key) = crate::subscription_catalog::configured_api_key() else {
        anyhow::bail!(
            "No Jcode account credential is configured. Run `jcode account login` to sign in."
        );
    };
    let client = crate::provider::shared_http_client();
    let api_base = subscription_api::configured_api_base();
    match subscription_api::fetch_subscription_me_with(&client, &api_base, &api_key).await {
        Ok(me) if json => {
            println!("{}", serde_json::to_string_pretty(&me)?);
            Ok(())
        }
        Ok(me) => {
            let tier = me
                .parsed_tier()
                .map(|tier| tier.display_name().to_string())
                .unwrap_or_else(|| me.tier.clone());
            println!("Jcode Account");
            println!("  Email:  {}", me.email);
            println!("  Plan:   {} ({})", tier, me.status);
            println!(
                "  Usage:  ${:.2} of ${:.2}",
                me.usage.used_usd, me.usage.budget_usd
            );
            if let Some(resets_at) = me.usage.resets_at {
                println!("  Resets: {}", resets_at);
            }
            println!("\nManage: {}", public_manage_url(me.manage_url.as_deref()));
            Ok(())
        }
        Err(AccountApiError::Unauthorized) => {
            crate::subscription_catalog::clear_account_credentials()
                .context("The account key is revoked, and local credential cleanup failed")?;
            anyhow::bail!(
                "The Jcode account key was revoked or expired. Local credentials were cleared. Run `jcode account login` to sign in again."
            )
        }
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

pub(crate) fn run_manage() -> Result<()> {
    let url = crate::subscription_catalog::JCODE_ACCOUNT_URL;
    println!("Opening Jcode account management: {url}");
    if crate::auth::browser_suppressed(false) {
        println!("Browser launch is disabled. Open the URL above manually.");
        return Ok(());
    }
    open::that_detached(url)
        .with_context(|| format!("Could not open the browser. Open {url} manually instead."))?;
    Ok(())
}

pub(crate) async fn run_logout() -> Result<()> {
    let api_key = crate::subscription_catalog::configured_api_key();
    let remote = if let Some(api_key) = api_key.as_deref() {
        subscription_api::revoke_current_key(
            &crate::provider::shared_http_client(),
            &subscription_api::configured_api_base(),
            api_key,
        )
        .await
    } else {
        Ok(())
    };

    // Local cleanup is unconditional, including offline and already-revoked
    // cases. This is the security boundary the CLI can always enforce.
    crate::subscription_catalog::clear_account_credentials()
        .context("Failed to securely clear local Jcode account credentials")?;
    crate::auth::AuthStatus::invalidate_cache();

    match (api_key.is_some(), remote) {
        (false, _) => {
            println!("No local Jcode account credential was present. Local account cache is clear.")
        }
        (true, Ok(())) => println!(
            "Jcode account key revoked. Local credentials and account cache were securely cleared."
        ),
        (true, Err(AccountApiError::Unauthorized)) => println!(
            "The Jcode account key was already revoked. Local credentials and account cache were securely cleared."
        ),
        (true, Err(AccountApiError::Offline(_))) => println!(
            "Local credentials and account cache were securely cleared. The account API was offline, so remote key revocation could not be confirmed."
        ),
        (true, Err(error)) => println!(
            "Local credentials and account cache were securely cleared. Remote key revocation could not be confirmed: {error}"
        ),
    }
    Ok(())
}

fn public_manage_url(candidate: Option<&str>) -> &str {
    candidate
        .filter(|url| {
            matches!(
                reqwest::Url::parse(url),
                Ok(parsed)
                    if parsed.scheme() == "https"
                        && matches!(parsed.host_str(), Some("jcode.sh" | "www.jcode.sh" | "solosystems.dev"))
                        && parsed.username().is_empty()
                        && parsed.password().is_none()
            )
        })
        .unwrap_or(crate::subscription_catalog::JCODE_ACCOUNT_URL)
}

pub(crate) async fn run_mobile(action: super::args::MobileCommand) -> Result<()> {
    use super::args::MobileCommand;
    use crate::mobile_sync;
    let client = crate::provider::shared_http_client();
    match action {
        MobileCommand::Enable => {
            let Some(api_key) = crate::subscription_catalog::configured_api_key() else {
                anyhow::bail!(
                    "Mobile sync needs a Jcode account. Run `jcode account login` first."
                );
            };
            let api_base = subscription_api::configured_api_base();
            let me = subscription_api::fetch_subscription_me_with(&client, &api_base, &api_key)
                .await
                .map_err(|err| anyhow::anyhow!("could not check your Jcode account: {err}"))?;
            if !me.capabilities.mobile_sync {
                anyhow::bail!(
                    "Mobile sync requires an active Jcode subscription. Subscribe at https://jcode.sh/pricing"
                );
            }
            let mut state = mobile_sync::load_state();
            state.enabled = true;
            mobile_sync::save_state(&state)?;
            println!("Mobile sync is on for {}.", me.email);
            println!("Uploading recent sessions…");
            let report = mobile_sync::sync_once(&client).await?;
            println!(
                "Synced {} sessions ({} messages{}). New activity syncs automatically while jcode runs.",
                report.sessions_uploaded,
                report.messages_uploaded,
                if report.errors > 0 {
                    format!(", {} errors", report.errors)
                } else {
                    String::new()
                }
            );
            println!("Sign in to the jcode iOS app with the same email to see them.");
        }
        MobileCommand::Disable => {
            mobile_sync::save_state(&mobile_sync::MobileSyncState::default())?;
            match mobile_sync::wipe_remote(&client).await {
                Ok(()) => println!("Mobile sync is off and stored transcripts were deleted."),
                Err(err) => println!(
                    "Mobile sync is off. Deleting stored transcripts failed ({err}); rerun `jcode mobile disable` to retry."
                ),
            }
        }
        MobileCommand::Status => {
            let state = mobile_sync::load_state();
            println!(
                "Mobile sync: {} ({} sessions tracked)",
                if state.enabled { "on" } else { "off" },
                state.sessions.len()
            );
        }
        MobileCommand::Sync => {
            let report = mobile_sync::sync_once(&client).await?;
            println!(
                "Synced {} sessions, {} messages, {} errors",
                report.sessions_uploaded, report.messages_uploaded, report.errors
            );
        }
        MobileCommand::Runner { once } => {
            crate::mobile_runner::ensure_sync_enabled();
            println!("Waiting for sessions from the jcode app (Ctrl-C to stop)…");
            loop {
                match crate::mobile_runner::run_one(&client).await {
                    Ok(true) => println!("Finished a phone command"),
                    Ok(false) if once => break,
                    Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                    Err(err) => {
                        eprintln!("phone command failed: {err:#}");
                        if once {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manage_url_accepts_only_public_allowlisted_https_origins() {
        assert_eq!(
            public_manage_url(Some("https://jcode.sh/account")),
            "https://jcode.sh/account"
        );
        assert_eq!(
            public_manage_url(Some("https://evil.example/?key=jck_live_secret")),
            crate::subscription_catalog::JCODE_ACCOUNT_URL
        );
        assert_eq!(
            public_manage_url(Some("https://user:pass@jcode.sh/account")),
            crate::subscription_catalog::JCODE_ACCOUNT_URL
        );
    }
}
