//! Detect credential identity changes made outside this server (another jcode
//! process, a CLI account switch, a manual edit of the auth files) and tell
//! every connected client via `BusEvent::CredentialsChanged`, so turns held on
//! the previous account's rate/usage limit resend promptly.
//!
//! Only account identity is fingerprinted (active label plus the account's
//! email/id), never tokens: routine token refresh rewrites the auth file and
//! must not look like an account swap.

use std::collections::BTreeMap;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CredentialIdentity {
    /// Identity the default (unpinned) route uses, external logins included.
    anthropic: Option<String>,
    openai: Option<String>,
    /// Every stored account: label -> email/id plus refresh fingerprint. A
    /// pinned session uses exactly one of these, so a relogin of that label
    /// must reach it even when the default is unchanged.
    anthropic_accounts: BTreeMap<String, String>,
    openai_accounts: BTreeMap<String, String>,
}

/// One credential change to publish: the provider, and the stored account
/// label when the change is to one account (`None` = the default route).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CredentialChange {
    pub provider: &'static str,
    pub account_label: Option<String>,
}

/// Short, non-reversible fingerprint of a secret. Never log or store the
/// secret itself.
fn secret_fingerprint(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(secret.trim().as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

fn jcode_anthropic_identity() -> Option<(String, String)> {
    let auth = crate::auth::claude::load_auth_file().ok()?;
    let label = crate::auth::claude::active_account_label()?;
    let account = auth.anthropic_accounts.iter().find(|a| a.label == label)?;
    Some((
        format!(
            "{}|{}|{}",
            label,
            account.email.as_deref().unwrap_or(""),
            account.subscription_type.as_deref().unwrap_or("")
        ),
        account.refresh.clone(),
    ))
}

/// Identity of the Claude credential requests actually use. A trusted
/// external login (Claude Code's `~/.claude/.credentials.json`) takes
/// precedence over jcode's accounts, so it must be watched too. For jcode
/// accounts the label/email identity ignores token refreshes. For an external
/// login the refresh token identifies the login, so a plain access-token
/// refresh of the same login is not reported as a change.
fn anthropic_identity() -> Option<String> {
    let jcode = jcode_anthropic_identity();
    match crate::auth::claude::load_credentials() {
        Ok(effective) => {
            if let Some((identity, refresh)) = &jcode
                && !refresh.is_empty()
                && *refresh == effective.refresh_token
            {
                return Some(identity.clone());
            }
            let key = if effective.refresh_token.is_empty() {
                &effective.access_token
            } else {
                &effective.refresh_token
            };
            Some(format!("effective|{}", secret_fingerprint(key)))
        }
        Err(_) => jcode.map(|(identity, _)| identity),
    }
}

fn jcode_openai_identity() -> Option<(String, String)> {
    let auth = crate::auth::codex::load_auth_file().ok()?;
    let label = crate::auth::codex::active_account_label()?;
    let account = auth.openai_accounts.iter().find(|a| a.label == label)?;
    Some((
        format!(
            "{}|{}|{}",
            label,
            account.email.as_deref().unwrap_or(""),
            account.account_id.as_deref().unwrap_or("")
        ),
        account.refresh_token.clone(),
    ))
}

/// Identity of the OpenAI OAuth credential requests actually use (jcode
/// account, trusted legacy Codex `auth.json`, or another external login).
fn openai_identity() -> Option<String> {
    let jcode = jcode_openai_identity();
    match crate::auth::codex::load_oauth_credentials() {
        Ok(effective) => {
            if let Some((identity, refresh)) = &jcode
                && !refresh.is_empty()
                && *refresh == effective.refresh_token
            {
                return Some(identity.clone());
            }
            if let Some(account_id) = effective.account_id.as_deref()
                && !account_id.is_empty()
            {
                return Some(format!("effective|account|{account_id}"));
            }
            let key = if effective.refresh_token.is_empty() {
                &effective.access_token
            } else {
                &effective.refresh_token
            };
            Some(format!("effective|{}", secret_fingerprint(key)))
        }
        Err(_) => jcode.map(|(identity, _)| identity),
    }
}

/// Stored-account identity: the email (or OpenAI account id) when known, so a
/// routine token refresh is not a change. Without one, fall back to a refresh
/// token fingerprint, the only thing that tells two logins apart.
fn stored_account_identity(profile: [Option<&str>; 2], refresh: &str) -> String {
    let known: Vec<&str> = profile
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    if known.is_empty() {
        format!("refresh|{}", secret_fingerprint(refresh))
    } else {
        known.join("|")
    }
}

/// Per-label identity of every stored Claude account.
fn anthropic_account_identities() -> BTreeMap<String, String> {
    crate::auth::claude::load_auth_file()
        .map(|auth| {
            auth.anthropic_accounts
                .into_iter()
                .map(|account| {
                    let identity =
                        stored_account_identity([account.email.as_deref(), None], &account.refresh);
                    (account.label, identity)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Per-label identity of every stored OpenAI account.
fn openai_account_identities() -> BTreeMap<String, String> {
    crate::auth::codex::load_auth_file()
        .map(|auth| {
            auth.openai_accounts
                .into_iter()
                .map(|account| {
                    let identity = stored_account_identity(
                        [account.email.as_deref(), account.account_id.as_deref()],
                        &account.refresh_token,
                    );
                    (account.label, identity)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn current_identity() -> CredentialIdentity {
    CredentialIdentity {
        anthropic: anthropic_identity(),
        openai: openai_identity(),
        anthropic_accounts: anthropic_account_identities(),
        openai_accounts: openai_account_identities(),
    }
}

/// Labels whose stored account identity was added, removed, or changed.
fn changed_labels(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut labels: Vec<String> = before
        .iter()
        .filter(|(label, identity)| after.get(*label) != Some(*identity))
        .map(|(label, _)| label.clone())
        .collect();
    for label in after.keys() {
        if !before.contains_key(label) {
            labels.push(label.clone());
        }
    }
    labels.sort();
    labels.dedup();
    labels
}

/// Every change between two snapshots: one per changed stored account label,
/// plus one label-less change when the default route's identity moved.
pub(super) fn changed_credentials(
    before: &CredentialIdentity,
    after: &CredentialIdentity,
) -> Vec<CredentialChange> {
    let mut changes = Vec::new();
    for (provider, default_changed, before_accounts, after_accounts) in [
        (
            "anthropic",
            before.anthropic != after.anthropic,
            &before.anthropic_accounts,
            &after.anthropic_accounts,
        ),
        (
            "openai",
            before.openai != after.openai,
            &before.openai_accounts,
            &after.openai_accounts,
        ),
    ] {
        let labels = changed_labels(before_accounts, after_accounts);
        if default_changed {
            changes.push(CredentialChange {
                provider,
                account_label: None,
            });
        }
        for label in labels {
            changes.push(CredentialChange {
                provider,
                account_label: Some(label),
            });
        }
    }
    changes
}

/// Providers whose account identity differs between two snapshots.
#[cfg(test)]
pub(super) fn changed_providers(
    before: &CredentialIdentity,
    after: &CredentialIdentity,
) -> Vec<&'static str> {
    let mut changed: Vec<&'static str> = changed_credentials(before, after)
        .into_iter()
        .map(|change| change.provider)
        .collect();
    changed.dedup();
    changed
}

/// Watched providers an in-process `CredentialsChanged` announcement already
/// covers. `None` means every provider.
fn announced_watch_providers(announced: Option<&str>) -> Vec<&'static str> {
    let Some(provider) = announced else {
        return vec!["anthropic", "openai"];
    };
    let provider = provider.trim().to_ascii_lowercase();
    if provider.starts_with("claude") || provider.starts_with("anthropic") {
        vec!["anthropic"]
    } else if provider.starts_with("openai") || provider.starts_with("codex") {
        vec!["openai"]
    } else {
        Vec::new()
    }
}

/// Providers whose change must be published after taking snapshot `now`.
/// `announced` is `Some(provider)` when the snapshot was triggered by an
/// in-process announcement for `provider` (`None` inside means all).
#[cfg(test)]
pub(super) fn providers_to_publish(
    last: &CredentialIdentity,
    now: &CredentialIdentity,
    announced: Option<Option<&str>>,
) -> Vec<&'static str> {
    let mut providers: Vec<&'static str> = changes_to_publish(last, now, announced)
        .into_iter()
        .map(|change| change.provider)
        .collect();
    providers.dedup();
    providers
}

/// Changes that must be published after taking snapshot `now`, one per
/// changed account label. See `providers_to_publish` for `announced`.
pub(super) fn changes_to_publish(
    last: &CredentialIdentity,
    now: &CredentialIdentity,
    announced: Option<Option<&str>>,
) -> Vec<CredentialChange> {
    let suppressed = announced.map(announced_watch_providers).unwrap_or_default();
    changed_credentials(last, now)
        .into_iter()
        .filter(|change| !suppressed.contains(&change.provider))
        .collect()
}

/// Drop local state tied to the previous login of `provider`, then tell every
/// client. Mirrors the in-process auth-change path, scoped to the provider
/// whose login changed: a relogin often reuses the label, so the old login's
/// usage snapshot and usage-limit marker would otherwise gate the new one.
#[cfg(test)]
fn apply_external_change(provider: &'static str) {
    apply_external_credential_change(&CredentialChange {
        provider,
        account_label: None,
    });
}

fn apply_external_credential_change(change: &CredentialChange) {
    let provider = change.provider;
    let account_label = change.account_label.clone();
    crate::logging::info(&format!(
        "Credential identity changed on disk for {provider} (account {}); notifying clients",
        account_label.as_deref().unwrap_or("default")
    ));
    crate::auth::AuthStatus::invalidate_cache();
    match provider {
        "anthropic" => {
            if account_label.is_none() {
                crate::usage::invalidate_active_anthropic_usage();
            }
            crate::provider::clear_claude_provider_unavailability_for_account_label(
                account_label.as_deref(),
            );
        }
        "openai" => {
            let label = account_label
                .clone()
                .or_else(crate::auth::codex::active_account_label);
            crate::provider::clear_openai_provider_unavailability_for_account_label(
                label.as_deref(),
            );
            crate::provider::clear_all_model_unavailability_for_account();
        }
        _ => {}
    }
    crate::bus::Bus::global().publish(crate::bus::BusEvent::CredentialsChanged {
        provider: Some(provider.to_string()),
        account_label,
    });
}

pub(super) fn spawn() {
    tokio::spawn(async move {
        let mut bus_rx = crate::bus::Bus::global().subscribe();
        let mut last = tokio::task::spawn_blocking(current_identity)
            .await
            .unwrap_or_default();
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            let announced: Option<Option<String>> = tokio::select! {
                _ = interval.tick() => None,
                event = bus_rx.recv() => match event {
                    Ok(crate::bus::BusEvent::CredentialsChanged { provider, .. }) => Some(provider),
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            let Ok(now) = tokio::task::spawn_blocking(current_identity).await else {
                continue;
            };
            // An in-process login/switch already announced its own provider's
            // change, so only that provider is skipped. Changes to other
            // providers in the same window are still published.
            let announced = announced.as_ref().map(|provider| provider.as_deref());
            for change in changes_to_publish(&last, &now, announced) {
                apply_external_credential_change(&change);
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
            ..Default::default()
        }
    }

    #[test]
    fn announcement_suppresses_only_its_own_provider() {
        let before = id(Some("claude-otter|a@x|max"), Some("openai-fox|b@x|acct1"));
        let both = id(Some("claude-fox|c@x|max"), Some("openai-fox|z@x|acct9"));
        // Poll tick: everything that changed is published.
        assert_eq!(
            providers_to_publish(&before, &both, None),
            vec!["anthropic", "openai"]
        );
        // In-process Claude login races an external Codex relogin.
        assert_eq!(
            providers_to_publish(&before, &both, Some(Some("claude"))),
            vec!["openai"]
        );
        assert_eq!(
            providers_to_publish(&before, &both, Some(Some("anthropic"))),
            vec!["openai"]
        );
        // And the reverse.
        assert_eq!(
            providers_to_publish(&before, &both, Some(Some("openai"))),
            vec!["anthropic"]
        );
        // A login for an unwatched provider covers neither.
        assert_eq!(
            providers_to_publish(&before, &both, Some(Some("groq"))),
            vec!["anthropic", "openai"]
        );
        // A provider-less announcement covers every provider.
        assert!(providers_to_publish(&before, &both, Some(None)).is_empty());
    }

    /// An external relogin under the same label must not inherit the previous
    /// login's usage-limit marker, or the precheck keeps skipping Claude.
    #[test]
    fn external_claude_change_clears_previous_login_state() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        let _ = &sandbox;
        crate::provider::record_provider_unavailable_for_account("claude", "usage limit");
        crate::provider::record_provider_unavailable_for_account("openai", "quota");
        assert!(crate::provider::provider_unavailability_detail_for_account("claude").is_some());

        apply_external_change("anthropic");
        assert!(
            crate::provider::provider_unavailability_detail_for_account("claude").is_none(),
            "old Claude login's marker must be cleared"
        );
        assert!(
            crate::provider::provider_unavailability_detail_for_account("openai").is_some(),
            "an unchanged provider keeps its marker"
        );

        apply_external_change("openai");
        assert!(crate::provider::provider_unavailability_detail_for_account("openai").is_none());
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

    /// A pinned window uses one stored account, not the default. A relogin
    /// of that account (same label, different subscription) must be reported
    /// with its label, even though the default route's identity is unchanged.
    /// A token refresh of any stored account is not a change.
    #[test]
    fn credential_watch_reports_changed_label() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        let _ = &sandbox;
        let account =
            |label: &str, access: &str, email: &str| crate::auth::claude::AnthropicAccount {
                label: label.to_string(),
                access: access.to_string(),
                refresh: format!("refresh-{access}"),
                expires: 1,
                email: Some(email.to_string()),
                subscription_type: Some("max".to_string()),
                scopes: Vec::new(),
            };
        crate::auth::claude::upsert_account(account("claude-otter", "o1", "otter@x"))
            .expect("save");
        crate::auth::claude::upsert_account(account("claude-fox", "f1", "fox@x")).expect("save");
        let first = current_identity();

        // Token refresh of the non-default account: nothing to report.
        crate::auth::claude::upsert_account(account("claude-fox", "f2", "fox@x")).expect("save");
        assert!(changed_credentials(&first, &current_identity()).is_empty());

        // Relogin of claude-fox to another subscription. otter is still the
        // default, so only the fox label changed.
        crate::auth::claude::upsert_account(account("claude-fox", "f3", "new-fox@x"))
            .expect("save");
        assert_eq!(
            changes_to_publish(&first, &current_identity(), None),
            vec![CredentialChange {
                provider: "anthropic",
                account_label: Some("claude-fox".to_string()),
            }]
        );
        // The provider-level summary still reports Claude.
        assert_eq!(
            providers_to_publish(&first, &current_identity(), None),
            vec!["anthropic"]
        );

        // Adding a third account reports that label only.
        let before_add = current_identity();
        crate::auth::claude::upsert_account(account("claude-panda", "p1", "panda@x"))
            .expect("save");
        assert_eq!(
            changed_credentials(&before_add, &current_identity()),
            vec![CredentialChange {
                provider: "anthropic",
                account_label: Some("claude-panda".to_string()),
            }]
        );
    }

    fn write_claude_code_login(path: &std::path::Path, access: &str, refresh: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let expires = chrono::Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000;
        std::fs::write(
            path,
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": access,
                    "refreshToken": refresh,
                    "expiresAt": expires,
                    "scopes": ["user:inference"],
                    "subscriptionType": "max"
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    /// Claude Code rewrites its trusted credentials file on its own relogin.
    /// jcode's auth.json is untouched, yet requests now use a different
    /// account, so the watcher must report it. A plain access-token refresh of
    /// the same login must not be reported.
    #[test]
    fn external_claude_code_relogin_is_an_identity_change() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        let path = sandbox.external_dir().join(".claude/.credentials.json");
        write_claude_code_login(&path, "cc-access-A", "cc-refresh-A");
        crate::auth::claude::trust_external_auth_source(
            crate::auth::claude::ExternalClaudeAuthSource::ClaudeCode,
        )
        .expect("trust");
        let first = current_identity();
        assert!(first.anthropic.is_some());

        write_claude_code_login(&path, "cc-access-A2", "cc-refresh-A");
        assert!(
            changed_providers(&first, &current_identity()).is_empty(),
            "access-token refresh of the same login is not an account change"
        );

        write_claude_code_login(&path, "cc-access-B", "cc-refresh-B");
        assert_eq!(
            changed_providers(&first, &current_identity()),
            vec!["anthropic"]
        );
        let serialized = format!("{:?}", current_identity());
        assert!(!serialized.contains("cc-refresh-B") && !serialized.contains("cc-access-B"));
    }

    /// Same for a trusted legacy Codex `~/.codex/auth.json` login.
    #[test]
    fn external_codex_relogin_is_an_identity_change() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        let path = sandbox.external_dir().join(".codex/auth.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let write = |access: &str, refresh: &str| {
            std::fs::write(
                &path,
                serde_json::json!({
                    "tokens": {
                        "access_token": access,
                        "refresh_token": refresh,
                        "expires_at": chrono::Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000
                    }
                })
                .to_string(),
            )
            .unwrap();
        };
        write("codex-access-A", "codex-refresh-A");
        crate::auth::codex::trust_legacy_auth_for_future_use().expect("trust");
        let first = current_identity();
        assert!(first.openai.is_some());

        write("codex-access-A2", "codex-refresh-A");
        assert!(changed_providers(&first, &current_identity()).is_empty());

        write("codex-access-B", "codex-refresh-B");
        assert_eq!(
            changed_providers(&first, &current_identity()),
            vec!["openai"]
        );
    }
}
