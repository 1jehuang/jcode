use super::*;
use crate::auth::claude_cli_completion::LoginFlow;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

pub(super) async fn wait_for_login(
    flow: Weak<LoginFlow>,
    cancelled: Weak<AtomicBool>,
    timeout: std::time::Duration,
) -> Option<Result<bool, String>> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cancelled
            .upgrade()
            .is_none_or(|cancelled| cancelled.load(Ordering::Acquire))
        {
            return None;
        }
        let result = match flow.upgrade() {
            Some(flow) => flow.result(),
            None => return None,
        };
        match result {
            Ok(Some(completed)) => return Some(Ok(completed.is_some())),
            Err(error) => return Some(Err(error.to_string())),
            Ok(None) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Some(Err(
                "Claude Code login did not report completion. Retry /login or choose Jcode OAuth."
                    .into(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Activation {
    Waiting,
    Queued,
    Sent {
        id: u64,
        model_ack: bool,
        grant_verified: bool,
    },
    Confirmed,
    Failed(String),
}

impl App {
    pub(super) fn claude_cli_route_snapshot(
        &self,
    ) -> (
        String,
        String,
        Option<String>,
        Option<jcode_provider_core::ResolvedCredential>,
    ) {
        if self.is_remote {
            return (
                self.remote_provider_name
                    .clone()
                    .unwrap_or_else(|| self.provider.name().into()),
                self.remote_provider_model
                    .clone()
                    .unwrap_or_else(|| self.provider.model()),
                self.remote_provider_oauth_account_label.clone(),
                self.remote_resolved_credential,
            );
        }
        (
            self.provider.name().into(),
            self.provider.model(),
            self.provider.oauth_account_label(),
            self.provider.active_explicit_credential(),
        )
    }

    pub(super) fn track_claude_cli_login(&mut self) -> anyhow::Result<Arc<LoginFlow>> {
        if let Some(previous) = self.pending_login.take() {
            self.cancel_claude_cli_selection(&previous);
        }
        let flow = Arc::new(LoginFlow::new()?);
        let attempt = flow.attempt.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let weak = Arc::downgrade(&cancelled);
        let weak_flow = Arc::downgrade(&flow);
        self.begin_pending_login(PendingLogin::ClaudeCli {
            attempt: attempt.clone(),
            flow: Arc::clone(&flow),
            activation: Activation::Waiting,
            cancelled,
            expected_override: crate::auth::claude::get_active_account_override(),
            expected_route: self.claude_cli_route_snapshot(),
        });
        tokio::spawn(async move {
            if let Some(result) =
                wait_for_login(weak_flow, weak.clone(), std::time::Duration::from_secs(600)).await
            {
                let (success, message) = match result {
                    Ok(true) => (true, "Claude Code login is ready in Jcode.".into()),
                    Ok(false) => (
                        false,
                        "Claude Code sign-in failed. Check the login terminal and retry /login."
                            .into(),
                    ),
                    Err(message) => (false, message),
                };
                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                    provider: format!("claude_cli#{attempt}"),
                    success,
                    message,
                }));
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                if weak
                    .upgrade()
                    .is_some_and(|cancelled| !cancelled.load(Ordering::Acquire))
                {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted { provider: format!("claude_cli#{attempt}"), success: false, message: "The daemon did not confirm the completed Claude login. Retry the account selection.".into() }));
                }
            }
        });
        Ok(flow)
    }

    pub(in crate::tui::app) fn accept_claude_cli_completion(
        &mut self,
        login: &mut LoginCompleted,
    ) -> bool {
        let Some(attempt) = login.provider.strip_prefix("claude_cli#") else {
            return true;
        };
        let (flow, expected_override, expected_route, activation) = match &self.pending_login {
            Some(PendingLogin::ClaudeCli {
                attempt: pending,
                cancelled,
                flow,
                expected_override,
                expected_route,
                activation,
            }) if pending == attempt && !cancelled.load(Ordering::Acquire) => (
                Arc::clone(flow),
                expected_override.clone(),
                expected_route.clone(),
                activation.clone(),
            ),
            _ => return false,
        };
        if login.success {
            let result = (|| -> anyhow::Result<bool> {
                if matches!(activation, Activation::Queued | Activation::Sent { .. }) {
                    return Ok(false);
                }
                if let Activation::Failed(error) = &activation {
                    anyhow::bail!("{error}");
                }
                anyhow::ensure!(
                    !self.claude_cli_selection_pending(),
                    "Another model or account choice is pending"
                );
                anyhow::ensure!(
                    if matches!(activation, Activation::Confirmed) {
                        let current = self.claude_cli_route_snapshot();
                        current.0 == expected_route.0
                            && current.1 == expected_route.1
                            && flow
                                .result()?
                                .flatten()
                                .is_some_and(|completed| current.2 == completed.saved_account)
                            && current.3 == Some(jcode_provider_core::ResolvedCredential::Oauth)
                    } else {
                        self.claude_cli_route_snapshot() == expected_route
                    },
                    "Model or account selection changed while sign-in was running; select the completed login manually"
                );
                let completed = flow
                    .result()?
                    .flatten()
                    .ok_or_else(|| anyhow::anyhow!("This CLI login has not reported success"))?;
                crate::auth::claude_cli_completion::apply(
                    &completed,
                    if matches!(activation, Activation::Confirmed) {
                        completed.saved_account.as_deref()
                    } else {
                        expected_override.as_deref()
                    },
                )?;
                if self.is_remote
                    && !matches!(activation, Activation::Confirmed)
                    && matches!(
                        expected_route.0.to_ascii_lowercase().as_str(),
                        "claude" | "anthropic"
                    )
                    && expected_route.3 != Some(jcode_provider_core::ResolvedCredential::ApiKey)
                {
                    self.pending_route_selection = Some(crate::provider::RouteSelection {
                        model: expected_route.1.clone(),
                        runtime_key: jcode_provider_core::RuntimeKey::ClaudeOAuth,
                        api_method: completed
                            .saved_account
                            .as_ref()
                            .map(|label| format!("claude-oauth@{label}"))
                            .unwrap_or_else(|| "claude-oauth".into()),
                        provider_label: "Claude".into(),
                        detail: String::new(),
                    });
                    if let Some(PendingLogin::ClaudeCli { activation, .. }) =
                        &mut self.pending_login
                    {
                        *activation = Activation::Queued;
                    }
                    self.set_status_notice(
                        "Claude login: waiting for the daemon to confirm the account",
                    );
                    return Ok(false);
                }
                let activation = (|| -> anyhow::Result<()> {
                    // A Claude subscription window follows the login it requested;
                    // explicit API billing and other provider routes retain their choice.
                    if !self.is_remote
                        && matches!(
                            self.provider.name().to_ascii_lowercase().as_str(),
                            "claude" | "anthropic"
                        )
                        && self.provider.active_explicit_credential()
                            != Some(jcode_provider_core::ResolvedCredential::ApiKey)
                    {
                        let selection = crate::provider::RouteSelection {
                            model: self.provider.model(),
                            runtime_key: jcode_provider_core::RuntimeKey::ClaudeOAuth,
                            api_method: completed.saved_account.as_ref().map_or_else(
                                || "claude-oauth".into(),
                                |label| format!("claude-oauth@{label}"),
                            ),
                            provider_label: "Claude".into(),
                            detail: String::new(),
                        };
                        self.provider
                            .set_route_selection_verified(&selection, &completed.fingerprint)?;
                        self.finalize_model_switch(&selection.routed_model_spec());
                    }
                    Ok(())
                })();
                if activation.is_err()
                    && crate::auth::claude::get_active_account_override() == completed.saved_account
                {
                    crate::auth::claude::set_active_account_override(expected_override.clone());
                }
                activation.map(|_| true)
            })();
            match result {
                Ok(false) => return false,
                Ok(true) => {}
                Err(error) => {
                    login.success = false;
                    login.message = error.to_string();
                }
            }
        }
        if !login.success {
            self.rollback_claude_cli_selection();
        }
        login.provider = "claude".into();
        true
    }

    pub(super) fn handle_claude_cli_input(&mut self, pending: PendingLogin) {
        let ready = match &pending {
            PendingLogin::ClaudeCli { attempt, flow, .. } => match flow.result() {
                Ok(Some(completed)) => Some(LoginCompleted {
                    provider: format!("claude_cli#{attempt}"),
                    success: completed.is_some(),
                    message: "Claude Code login command completed.".into(),
                }),
                _ => None,
            },
            _ => None,
        };
        self.pending_login = Some(pending);
        if let Some(login) = ready {
            self.handle_login_completed(login);
        } else {
            self.set_status_notice(
                "Complete the displayed sign-in command in the terminal; /cancel stops waiting",
            );
        }
    }
}
