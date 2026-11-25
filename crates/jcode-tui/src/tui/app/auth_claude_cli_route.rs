use super::claude_cli_wait::Activation;
use super::*;

impl App {
    pub(super) fn claude_cli_selection_pending(&self) -> bool {
        self.pending_model_switch.is_some()
            || self.pending_route_selection.is_some()
            || self.pending_account_picker_action.is_some()
            || self.pending_account_switch_target.is_some()
            || self.pending_automatic_failover_route.is_some()
            || self.pending_automatic_failover_request_id.is_some()
            || self.remote_model_switch_in_flight
    }

    pub(in crate::tui::app) fn claude_cli_route_queued(&self) -> bool {
        matches!(
            &self.pending_login,
            Some(PendingLogin::ClaudeCli {
                activation: Activation::Queued,
                ..
            })
        )
    }

    pub(in crate::tui::app) fn claude_cli_route_verification(
        &self,
    ) -> anyhow::Result<Option<jcode_provider_core::AuthRouteVerification>> {
        let Some(PendingLogin::ClaudeCli {
            flow,
            attempt,
            expected_route,
            activation: Activation::Queued,
            ..
        }) = &self.pending_login
        else {
            return Ok(None);
        };
        let completed = flow
            .result()?
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("Claude sign-in result is missing"))?;
        Ok(Some(jcode_provider_core::AuthRouteVerification {
            fingerprint: completed.fingerprint,
            completion_path: flow.path.clone(),
            attempt: attempt.clone(),
            previous_provider: expected_route.0.clone(),
            previous_model: expected_route.1.clone(),
            previous_serving_account_label: expected_route.2.clone(),
            previous_credential: expected_route.3,
        }))
    }

    pub(in crate::tui::app) fn note_claude_cli_route_sent(&mut self, id: u64) {
        self.remote_model_switch_in_flight = true;
        if let Some(PendingLogin::ClaudeCli { activation, .. }) = &mut self.pending_login
            && matches!(activation, Activation::Queued)
        {
            *activation = Activation::Sent {
                id,
                model_ack: false,
                grant_verified: false,
            };
        }
    }

    fn publish_claude_cli_activation(&mut self, result: anyhow::Result<()>) {
        let Some(PendingLogin::ClaudeCli {
            attempt,
            activation,
            ..
        }) = &mut self.pending_login
        else {
            return;
        };
        let (success, message) = match result {
            Ok(()) => {
                *activation = Activation::Confirmed;
                (true, "Claude login account confirmed by the daemon.".into())
            }
            Err(error) => {
                let message = error.to_string();
                *activation = Activation::Failed(message.clone());
                (false, message)
            }
        };
        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
            provider: format!("claude_cli#{attempt}"),
            success,
            message,
        }));
    }

    pub(in crate::tui::app) fn fail_claude_cli_route_send(&mut self, error: &str) {
        self.set_status_notice("Model switch failed");
        if self.claude_cli_route_queued() {
            self.publish_claude_cli_activation(Err(anyhow::anyhow!("{error}")));
        }
    }

    pub(in crate::tui::app) fn remember_claude_cli_route_error(
        &mut self,
        id: u64,
        error: Option<String>,
    ) -> Option<String> {
        if let Some(PendingLogin::ClaudeCli {
            activation: Activation::Sent { id: expected, .. },
            ..
        }) = &self.pending_login
            && *expected == id
            && let Some(message) = &error
        {
            self.publish_claude_cli_activation(Err(anyhow::anyhow!("{message}")));
        }
        error
    }

    pub(in crate::tui::app) fn confirm_claude_cli_grant(
        &mut self,
        id: u64,
        verified: bool,
    ) -> bool {
        if let Some(PendingLogin::ClaudeCli {
            activation:
                Activation::Sent {
                    id: expected,
                    grant_verified,
                    ..
                },
            ..
        }) = &mut self.pending_login
            && *expected == id
        {
            if !verified {
                self.publish_claude_cli_activation(Err(anyhow::anyhow!(
                    "The daemon could not verify the completed Claude login"
                )));
            } else {
                *grant_verified = true;
                self.finish_claude_cli_route_ack(id);
            }
        }
        true
    }

    pub(in crate::tui::app) fn note_claude_cli_model_ack(&mut self, id: u64) -> bool {
        if let Some(PendingLogin::ClaudeCli {
            activation:
                Activation::Sent {
                    id: expected,
                    model_ack,
                    ..
                },
            ..
        }) = &mut self.pending_login
            && *expected == id
        {
            *model_ack = true;
        }
        self.finish_claude_cli_route_ack(id)
    }

    pub(in crate::tui::app) fn finish_claude_cli_route_ack(&mut self, id: u64) -> bool {
        let Some(PendingLogin::ClaudeCli {
            activation:
                Activation::Sent {
                    id: expected,
                    model_ack,
                    grant_verified,
                },
            ..
        }) = &mut self.pending_login
        else {
            return true;
        };
        if *expected != id {
            return true;
        }
        if !*model_ack || !*grant_verified {
            return true;
        }
        let result = if self.claude_cli_selection_pending() {
            Err(anyhow::anyhow!(
                "Another model or account choice is pending"
            ))
        } else {
            let Some(PendingLogin::ClaudeCli {
                flow,
                expected_route,
                ..
            }) = &self.pending_login
            else {
                return true;
            };
            (|| -> anyhow::Result<()> {
                let completed = flow
                    .result()?
                    .flatten()
                    .ok_or_else(|| anyhow::anyhow!("CLI login result is missing"))?;
                anyhow::ensure!(
                    self.remote_provider_model.as_deref() == Some(expected_route.1.as_str())
                        && self
                            .remote_provider_name
                            .as_deref()
                            .and_then(jcode_provider_core::parse_provider_hint)
                            == Some(jcode_provider_core::ActiveProvider::Claude)
                        && self.remote_provider_oauth_account_label == completed.saved_account
                        && self.remote_resolved_credential
                            == Some(jcode_provider_core::ResolvedCredential::Oauth),
                    "The daemon did not confirm the requested Claude account and model"
                );
                Ok(())
            })()
        };
        self.publish_claude_cli_activation(result);
        true
    }

    fn restore_claude_cli_selection(&mut self, pending: &PendingLogin) {
        if let PendingLogin::ClaudeCli {
            flow,
            expected_override,
            activation,
            ..
        } = pending
            && !matches!(activation, Activation::Waiting)
            && let Ok(Some(Some(completed))) = flow.result()
        {
            if crate::auth::claude::get_active_account_override() == completed.saved_account {
                crate::auth::claude::set_active_account_override(expected_override.clone());
            }
            if self
                .pending_route_selection
                .as_ref()
                .is_some_and(|selection| {
                    selection.runtime_key == jcode_provider_core::RuntimeKey::ClaudeOAuth
                        && selection.account_label() == completed.saved_account.as_deref()
                })
            {
                self.pending_route_selection = None;
            }
        }
    }

    pub(super) fn rollback_claude_cli_selection(&mut self) {
        if let Some(pending @ PendingLogin::ClaudeCli { .. }) = self.pending_login.clone() {
            self.cancel_claude_cli_selection(&pending);
        }
    }

    pub(super) fn cancel_claude_cli_selection(&mut self, pending: &PendingLogin) {
        if let PendingLogin::ClaudeCli { cancelled, .. } = pending {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
            self.restore_claude_cli_selection(pending);
            if let PendingLogin::ClaudeCli { flow, .. } = pending
                && let Err(error) = flow.cancel()
            {
                crate::logging::warn(&format!("Could not cancel Claude login receipt: {error}"));
            }
        }
    }
}
