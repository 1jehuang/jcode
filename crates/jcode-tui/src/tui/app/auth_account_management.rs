use super::*;

/// Validate before persisting: a browser session must never silently replace a
/// saved login with another identity, including when identity cannot be proven.
fn require_matching_identity(expected: Option<&str>, actual: Option<&str>) -> Result<(), String> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if expected.eq_ignore_ascii_case(actual) => Ok(()),
        _ => Err("The browser login does not match the saved account, or its identity could not be verified. No credentials were replaced. Choose the original account in your browser. To intentionally change this saved login, choose Replace login in /accounts; to keep both identities, add a separate login.".to_string()),
    }
}

impl App {
    fn saved_login_origin_session_id(&self) -> &str {
        if self.is_remote {
            self.remote_session_id
                .as_deref()
                .unwrap_or(&self.session.id)
        } else {
            &self.session.id
        }
    }

    pub(super) fn saved_login_request_id(verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(verifier.as_bytes()))
    }

    pub(super) fn saved_login_completion(
        &self,
        provider: &str,
        label: &str,
        verifier: &str,
        old_identity: Option<String>,
        replace: bool,
        attempt: std::sync::Arc<std::sync::atomic::AtomicU8>,
    ) -> crate::bus::SavedAccountLoginCompleted {
        crate::bus::SavedAccountLoginCompleted {
            origin_session_id: self.saved_login_origin_session_id().to_string(),
            request_id: Self::saved_login_request_id(verifier),
            label: label.to_string(),
            old_identity,
            new_identity: None,
            replace,
            commit_claimed: false,
            exchange_claimed: false,
            attempt,
            login: crate::bus::LoginCompleted {
                provider: provider.to_string(),
                success: false,
                message: String::new(),
            },
        }
    }

    pub(super) fn begin_saved_login_exchange(
        completion: &mut crate::bus::SavedAccountLoginCompleted,
    ) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        completion.attempt.compare_exchange(0, 4, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| { completion.exchange_claimed = true; })
            .map_err(|_| "This login was cancelled or another callback already started it. No credentials were replaced.".to_string())
    }

    pub(super) fn claim_saved_login_commit(
        completion: &mut crate::bus::SavedAccountLoginCompleted,
    ) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        completion.attempt.compare_exchange(4, 1, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| { completion.commit_claimed = true; })
            .map_err(|_| "This login was cancelled or another callback already completed it. No credentials were replaced.".to_string())
    }

    pub(super) fn publish_saved_login_completion(
        mut completion: crate::bus::SavedAccountLoginCompleted,
        result: Result<String, String>,
    ) {
        use std::sync::atomic::Ordering;
        if result.is_err()
            && !completion.commit_claimed
            && completion
                .attempt
                .compare_exchange(
                    if completion.exchange_claimed { 4 } else { 0 },
                    3,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            return;
        }
        completion.login.success = result.is_ok();
        completion.login.message = match result {
            Ok(message) | Err(message) => message,
        };
        crate::bus::Bus::global()
            .publish(crate::bus::BusEvent::SavedAccountLoginCompleted(completion));
    }

    /// Returns false for another window, cancelled flow, or a late callback.
    pub(in crate::tui::app) fn handle_saved_account_login_completed(
        &mut self,
        completion: crate::bus::SavedAccountLoginCompleted,
    ) -> bool {
        if completion.origin_session_id != self.saved_login_origin_session_id() {
            return false;
        }
        let matches = match self.pending_login.as_ref() {
            Some(PendingLogin::ClaudeAccount {
                verifier,
                label,
                expected_identity,
                replace,
                attempt,
                ..
            }) if completion.login.provider == "claude" => {
                label == &completion.label
                    && expected_identity == &completion.old_identity
                    && *replace == completion.replace
                    && Self::saved_login_request_id(verifier) == completion.request_id
                    && std::sync::Arc::ptr_eq(attempt, &completion.attempt)
            }
            Some(PendingLogin::OpenAiAccount {
                verifier,
                label,
                expected_identity,
                replace,
                attempt,
                ..
            }) if completion.login.provider == "openai" => {
                label == &completion.label
                    && expected_identity == &completion.old_identity
                    && *replace == completion.replace
                    && Self::saved_login_request_id(verifier) == completion.request_id
                    && std::sync::Arc::ptr_eq(attempt, &completion.attempt)
            }
            _ => false,
        };
        if !matches
            || completion
                .attempt
                .load(std::sync::atomic::Ordering::Acquire)
                == 2
        {
            return false;
        }
        if let Some((provider, method)) = self
            .pending_login
            .as_ref()
            .and_then(PendingLogin::telemetry_context)
        {
            if completion.login.success {
                crate::telemetry::record_auth_success(&provider, &method);
            } else {
                let reason = crate::auth::login_diagnostics::classify_auth_failure_message(
                    &completion.login.message,
                );
                crate::telemetry::record_auth_failed_reason(&provider, &method, reason.label());
            }
        }
        if completion.login.success && completion.replace && !self.is_remote {
            let provider_id = completion.login.provider.as_str();
            // A later replacement may have won before this event was drained.
            if Self::saved_login_snapshot(provider_id, &completion.label).ok()
                == Some((true, completion.new_identity.clone()))
            {
                let old = crate::auth::AccountPin::new(
                    &completion.label,
                    completion.old_identity.clone(),
                );
                let new = crate::auth::AccountPin::new(
                    &completion.label,
                    completion.new_identity.clone(),
                );
                if let Some(kind) = jcode_provider_core::AccountProviderKind::from_key(provider_id)
                {
                    match crate::session_accounts::migrate_pin_identity(
                        self.provider.as_ref(),
                        &mut self.session,
                        kind,
                        &old,
                        &new,
                    ) {
                        Ok(true) => {
                            if let Err(error) = self.session.save() {
                                crate::logging::warn(&format!(
                                    "Login replaced but session pins could not be saved: {error}"
                                ));
                            }
                        }
                        Ok(false) => {}
                        Err(error) => crate::logging::warn(&format!(
                            "Login replaced but session pins could not be migrated: {error}"
                        )),
                    }
                }
            }
        }
        self.pending_login = None;
        self.handle_login_completed(completion.login);
        true
    }

    pub(in crate::tui::app) fn saved_login_snapshot(
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

    pub(super) fn verify_login_target_snapshot(
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

    /// Start replacement only after the account manager has confirmed the action.
    pub(in crate::tui::app) fn replace_saved_account(&mut self, provider_id: &str, label: &str) {
        match Self::saved_login_snapshot(provider_id, label) {
            Ok((true, _)) => match provider_id {
                "claude" => self.start_claude_login_for_account_mode(label, true),
                "openai" => self.start_openai_login_for_account_mode(label, true),
                _ => unreachable!(),
            },
            Ok((false, _)) => self.push_display_message(DisplayMessage::error(format!(
                "No saved {provider_id} account named {label}. No credentials were replaced."
            ))),
            Err(error) => self.push_display_message(DisplayMessage::error(error)),
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

    fn attempt() -> (super::PendingLogin, crate::bus::SavedAccountLoginCompleted) {
        let guard = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let pending = super::PendingLogin::ClaudeAccount {
            verifier: "test-verifier".to_string(),
            label: "work".to_string(),
            expected_existing: true,
            expected_identity: Some("old@example.com".to_string()),
            replace: true,
            attempt: guard.clone(),
            redirect_uri: None,
        };
        let completion = crate::bus::SavedAccountLoginCompleted {
            origin_session_id: "origin".to_string(),
            request_id: super::App::saved_login_request_id("test-verifier"),
            label: "work".to_string(),
            old_identity: Some("old@example.com".to_string()),
            new_identity: None,
            replace: true,
            commit_claimed: false,
            exchange_claimed: false,
            attempt: guard,
            login: crate::bus::LoginCompleted {
                provider: "claude".to_string(),
                success: false,
                message: "failed".to_string(),
            },
        };
        (pending, completion)
    }

    #[test]
    fn saved_login_cancel_blocks_waiting_and_inflight_attempts_before_commit() {
        let (pending, mut completion) = attempt();
        assert!(!pending.is_exchanging());
        assert!(pending.cancel_saved_attempt());
        assert!(super::App::begin_saved_login_exchange(&mut completion).is_err());
        assert!(super::App::claim_saved_login_commit(&mut completion).is_err());

        let (pending, mut completion) = attempt();
        super::App::begin_saved_login_exchange(&mut completion).unwrap();
        assert!(pending.is_exchanging());
        assert!(pending.cancel_saved_attempt());
        assert!(!pending.is_exchanging());
        assert!(super::App::claim_saved_login_commit(&mut completion).is_err());
    }

    #[test]
    fn saved_login_callback_arbitration_has_one_exchange_and_commit_winner() {
        let (pending, mut first) = attempt();
        let mut second = first.clone();
        super::App::begin_saved_login_exchange(&mut first).unwrap();
        assert!(super::App::begin_saved_login_exchange(&mut second).is_err());
        super::App::claim_saved_login_commit(&mut first).unwrap();
        assert!(super::App::claim_saved_login_commit(&mut second).is_err());
        assert!(
            !pending.cancel_saved_attempt(),
            "cannot claim cancellation after commit won"
        );
        assert!(pending.is_exchanging());
    }

    #[test]
    fn saved_login_completion_ignores_other_window_and_newer_attempt() {
        let mut app = crate::tui::app::tests::create_test_app();
        let (pending, mut completion) = attempt();
        app.pending_login = Some(pending.clone());
        assert!(!app.handle_saved_account_login_completed(completion.clone()));
        assert!(app.pending_login.is_some());
        completion.origin_session_id = app.session.id.clone();
        completion.request_id = "old-attempt".to_string();
        assert!(!app.handle_saved_account_login_completed(completion));
        assert!(app.pending_login.is_some());
        assert!(pending.cancel_saved_attempt());
    }

    #[test]
    fn unrelated_generic_completion_does_not_clear_saved_oauth_attempt() {
        let mut app = crate::tui::app::tests::create_test_app();
        let (pending, _) = attempt();
        app.pending_login = Some(pending);
        app.handle_login_completed(crate::bus::LoginCompleted {
            provider: "openai".to_string(),
            success: false,
            message: "unrelated".to_string(),
        });
        assert!(app.pending_login.is_some());
    }

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
