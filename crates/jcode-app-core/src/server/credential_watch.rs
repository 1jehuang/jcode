//! Detect credential identity changes made outside this server (another jcode
//! process, a CLI account switch, a manual edit of the auth files) and tell
//! every connected client via `BusEvent::CredentialsChanged`, so turns held on
//! the previous account's rate/usage limit resend promptly.
//!
//! Only account identity is fingerprinted (active label plus the account's
//! email/id), never tokens: routine token refresh rewrites the auth file and
//! must not look like an account swap.

use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CredentialIdentity {
    anthropic: Option<String>,
    openai: Option<String>,
}

fn anthropic_identity() -> Option<String> {
    let auth = crate::auth::claude::load_auth_file().ok()?;
    let label = crate::auth::claude::active_account_label()?;
    let account = auth.anthropic_accounts.iter().find(|a| a.label == label)?;
    Some(format!(
        "{}|{}|{}",
        label,
        account.email.as_deref().unwrap_or(""),
        account.subscription_type.as_deref().unwrap_or("")
    ))
}

fn openai_identity() -> Option<String> {
    let auth = crate::auth::codex::load_auth_file().ok()?;
    let label = crate::auth::codex::active_account_label()?;
    let account = auth.openai_accounts.iter().find(|a| a.label == label)?;
    Some(format!(
        "{}|{}|{}",
        label,
        account.email.as_deref().unwrap_or(""),
        account.account_id.as_deref().unwrap_or("")
    ))
}

pub(super) fn current_identity() -> CredentialIdentity {
    CredentialIdentity {
        anthropic: anthropic_identity(),
        openai: openai_identity(),
    }
}

/// Providers whose account identity differs between two snapshots.
pub(super) fn changed_providers(
    before: &CredentialIdentity,
    after: &CredentialIdentity,
) -> Vec<&'static str> {
    let mut changed = Vec::new();
    if before.anthropic != after.anthropic {
        changed.push("anthropic");
    }
    if before.openai != after.openai {
        changed.push("openai");
    }
    changed
}

pub(super) fn spawn() {
    tokio::spawn(async move {
        let mut bus_rx = crate::bus::Bus::global().subscribe();
        let mut last = tokio::task::spawn_blocking(current_identity)
            .await
            .unwrap_or(CredentialIdentity {
                anthropic: None,
                openai: None,
            });
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            let announced = tokio::select! {
                _ = interval.tick() => false,
                event = bus_rx.recv() => match event {
                    Ok(crate::bus::BusEvent::CredentialsChanged { .. }) => true,
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            let Ok(now) = tokio::task::spawn_blocking(current_identity).await else {
                continue;
            };
            // An in-process login/switch already announced this change; only
            // move the baseline so the next poll does not announce it again.
            if !announced {
                for provider in changed_providers(&last, &now) {
                    crate::logging::info(&format!(
                        "Credential identity changed on disk for {provider}; notifying clients"
                    ));
                    crate::auth::AuthStatus::invalidate_cache();
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::CredentialsChanged {
                        provider: Some(provider.to_string()),
                    });
                }
            }
            last = now;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(anthropic: Option<&str>, openai: Option<&str>) -> CredentialIdentity {
        CredentialIdentity {
            anthropic: anthropic.map(str::to_string),
            openai: openai.map(str::to_string),
        }
    }

    #[test]
    fn identity_change_is_reported_per_provider() {
        let before = id(Some("claude-otter|a@x|max"), Some("openai-fox|b@x|acct1"));
        assert!(changed_providers(&before, &before.clone()).is_empty());
        let swapped = id(Some("claude-fox|c@x|max"), Some("openai-fox|b@x|acct1"));
        assert_eq!(changed_providers(&before, &swapped), vec!["anthropic"]);
        let relogin_same_label = id(Some("claude-otter|a@x|max"), Some("openai-fox|z@x|acct9"));
        assert_eq!(
            changed_providers(&before, &relogin_same_label),
            vec!["openai"]
        );
    }

    #[test]
    fn fingerprint_ignores_token_refresh() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        let _ = &sandbox;
        let account = |access: &str, email: &str| crate::auth::claude::AnthropicAccount {
            label: "claude-otter".to_string(),
            access: access.to_string(),
            refresh: format!("refresh-{access}"),
            expires: 1,
            email: Some(email.to_string()),
            subscription_type: Some("max".to_string()),
            scopes: Vec::new(),
        };
        crate::auth::claude::upsert_account(account("tok-1", "a@example.com")).expect("save");
        let first = current_identity();
        assert!(first.anthropic.is_some());

        // Token refresh: same account, new tokens. Not an account change.
        crate::auth::claude::upsert_account(account("tok-2", "a@example.com")).expect("save");
        assert!(changed_providers(&first, &current_identity()).is_empty());

        // Same label, different account (relogin to another subscription).
        crate::auth::claude::upsert_account(account("tok-3", "b@example.com")).expect("save");
        assert_eq!(
            changed_providers(&first, &current_identity()),
            vec!["anthropic"]
        );
    }
}
