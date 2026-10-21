use super::*;

/// Validate before persisting: a browser session must never silently replace a
/// saved login with another identity, including when identity cannot be proven.
fn require_matching_identity(expected: Option<&str>, actual: Option<&str>) -> Result<(), String> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if expected.eq_ignore_ascii_case(actual) => Ok(()),
        _ => Err("The browser login does not match the saved account, or its identity could not be verified. No credentials were replaced. Choose the original account in your browser, or add a separate login from /accounts.".to_string()),
    }
}

impl App {
    pub(super) fn saved_login_snapshot(
        provider_id: &str,
        label: &str,
    ) -> Result<(bool, Option<String>), String> {
        match provider_id {
            "claude" => {
                let account = crate::auth::claude::list_accounts()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|account| account.label == label);
                Ok((
                    account.is_some(),
                    account
                        .as_ref()
                        .and_then(crate::auth::claude::account_identity),
                ))
            }
            "openai" => {
                let account = crate::auth::codex::list_accounts()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|account| account.label == label);
                Ok((
                    account.is_some(),
                    account
                        .as_ref()
                        .and_then(crate::auth::codex::account_identity),
                ))
            }
            _ => Err(format!("Saved logins are not supported for {provider_id}")),
        }
    }

    fn verify_login_target_snapshot(
        provider_id: &str,
        label: &str,
        expected: &(bool, Option<String>),
    ) -> Result<(), String> {
        let actual = Self::saved_login_snapshot(provider_id, label)?;
        require_unchanged_target(expected, &actual)
    }

    pub(in crate::tui::app) fn rename_saved_account(
        &mut self,
        provider_id: &str,
        old_label: &str,
        new_label: &str,
    ) {
        let result = match provider_id {
            "claude" => crate::auth::claude::rename_account(old_label, new_label),
            "openai" => crate::auth::codex::rename_account(old_label, new_label),
            _ => Err(anyhow::anyhow!(
                "Saved account renaming is not supported for {provider_id}"
            )),
        };
        match result {
            Ok(()) => {
                crate::auth::AuthStatus::invalidate_cache();
                let new_label = new_label.trim();
                if let Some(kind) = jcode_provider_core::AccountProviderKind::from_key(provider_id)
                {
                    if let Some(pin) = self.session.account_pins.get_mut(provider_id)
                        && pin.label == old_label
                    {
                        pin.label = new_label.to_string();
                        let _ = self.provider.set_account_pin(kind, Some(pin.clone()));
                    }
                    if let Some(home) = self.session.account_failover_home.get_mut(provider_id)
                        && home.label == old_label
                    {
                        home.label = new_label.to_string();
                        self.provider
                            .set_account_failover_home(kind, Some(home.clone()));
                    }
                    for info in &mut self.window_accounts {
                        if info.provider == provider_id && info.label.as_deref() == Some(old_label)
                        {
                            info.label = Some(new_label.to_string());
                        }
                    }
                    if let Err(error) = self.session.save() {
                        crate::logging::warn(&format!(
                            "Account renamed but session metadata could not be saved: {error}"
                        ));
                    }
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Renamed {provider_id} account {old_label} to {}. Existing identity pins and the default selection are preserved.", new_label.trim()
                )));
                self.set_status_notice("Account renamed");
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Could not rename account: {error}"
                )));
                self.set_status_notice("Account rename failed");
            }
        }
    }

    pub(in crate::tui::app) fn reauthenticate_saved_account(
        &mut self,
        provider_id: &str,
        label: &str,
    ) {
        let result = match provider_id {
            "claude" => crate::auth::claude::list_accounts()
                .map(|accounts| accounts.iter().any(|a| a.label == label)),
            "openai" => crate::auth::codex::list_accounts()
                .map(|accounts| accounts.iter().any(|a| a.label == label)),
            _ => Err(anyhow::anyhow!(
                "Saved account reauthentication is not supported for {provider_id}"
            )),
        };
        match result {
            Ok(true) => match provider_id {
                "claude" => self.start_claude_login_for_account(label),
                "openai" => self.start_openai_login_for_account(label),
                _ => unreachable!(),
            },
            Ok(false) => self.push_display_message(DisplayMessage::error(format!(
                "No saved {provider_id} account named {label}. Open /accounts to select a saved login."
            ))),
            Err(error) => self.push_display_message(DisplayMessage::error(format!("Could not reauthenticate account: {error}"))),
        }
    }

    pub(super) async fn verify_saved_claude_login(
        label: &str,
        access_token: &str,
        target_snapshot: &(bool, Option<String>),
    ) -> Result<(), String> {
        Self::verify_login_target_snapshot("claude", label, target_snapshot)?;
        let existing = crate::auth::claude::list_accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|account| account.label == label);
        if let Some(existing) = existing {
            let email = crate::auth::oauth::fetch_claude_profile_email(access_token).await
                .map_err(|e| format!("Could not verify the saved login identity: {e}. No credentials were replaced."))?;
            require_matching_identity(existing.email.as_deref(), email.as_deref())?;
        }
        // The profile request awaited network I/O: check again in case another
        // window removed, renamed, or replaced the target while it was running.
        Self::verify_login_target_snapshot("claude", label, target_snapshot)?;
        Ok(())
    }

    pub(super) fn verify_saved_openai_login(
        label: &str,
        id_token: Option<&str>,
        target_snapshot: &(bool, Option<String>),
    ) -> Result<(), String> {
        Self::verify_login_target_snapshot("openai", label, target_snapshot)?;
        let existing = crate::auth::codex::list_accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|account| account.label == label);
        if let Some(existing) = existing {
            let expected_id = existing.account_id.clone().or_else(|| {
                existing
                    .id_token
                    .as_deref()
                    .and_then(crate::auth::codex::extract_account_id)
            });
            let actual_id = id_token.and_then(crate::auth::codex::extract_account_id);
            // A workspace ID is stronger than an email: the same email may own
            // multiple subscriptions, which must remain separate saved logins.
            if expected_id.is_some() {
                if expected_id != actual_id {
                    return require_matching_identity(None, None);
                }
            } else {
                let expected_email = existing.email.clone().or_else(|| {
                    existing
                        .id_token
                        .as_deref()
                        .and_then(crate::auth::codex::extract_email)
                });
                let actual_email = id_token.and_then(crate::auth::codex::extract_email);
                require_matching_identity(expected_email.as_deref(), actual_email.as_deref())?;
            }
        }
        Ok(())
    }
}

fn require_unchanged_target(
    expected: &(bool, Option<String>),
    actual: &(bool, Option<String>),
) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err("The saved login was removed, renamed, or replaced while authentication was pending. No credentials were replaced. Open /accounts and start again.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{require_matching_identity, require_unchanged_target};

    #[test]
    fn pending_login_rejects_deleted_replaced_or_newly_occupied_targets() {
        let original = (true, Some("original@example.com".to_string()));
        let removed = (false, None);
        let replaced = (true, Some("other@example.com".to_string()));
        assert!(require_unchanged_target(&original, &original).is_ok());
        assert!(require_unchanged_target(&original, &removed).is_err());
        assert!(require_unchanged_target(&original, &replaced).is_err());
        assert!(require_unchanged_target(&removed, &original).is_err());
        assert!(require_unchanged_target(&removed, &removed).is_ok());
    }

    #[test]
    fn saved_login_identity_accepts_same_email_case_insensitively() {
        assert!(
            require_matching_identity(Some("Person@Example.com"), Some("person@example.com"))
                .is_ok()
        );
    }

    #[test]
    fn saved_login_identity_rejects_other_and_unverifiable_accounts() {
        for (expected, actual) in [
            (Some("one"), Some("two")),
            (Some("one"), None),
            (None, Some("two")),
            (None, None),
        ] {
            assert!(
                require_matching_identity(expected, actual)
                    .unwrap_err()
                    .contains("No credentials were replaced")
            );
        }
    }
}
