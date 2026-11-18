//! Provider fallback and context recovery.

use super::*;

impl App {
    /// [`Self::offer_fallback_after_error`] with an explicitly captured resend
    /// payload, for callers whose error-cleanup already cleared the pending
    /// retry slot before the offer could be armed.
    pub(in super::super) fn offer_fallback_after_error_with_payload(
        &mut self,
        error: &str,
        remote_resend: Option<super::super::FallbackResendPayload>,
    ) -> bool {
        // Never compete with the automatic countdown switcher. A migration
        // lease block is not a provider failure: switching model cannot help.
        if self.pending_provider_failover.is_some()
            || self.automatic_failover_stopped
            || self.pending_automatic_failover_route.is_some()
            || self.pending_automatic_failover_request_id.is_some()
            || error.contains("Session is not runnable here")
        {
            return false;
        }
        let routes = self.fallback_candidate_routes();
        let current_model = self.current_model_for_fallback();
        let current_provider = self.current_provider_label_for_fallback();
        let current_api_method = self.current_route_api_method().unwrap_or_default();

        // A credential failure (expired OAuth session, failed token refresh,
        // broken API key) breaks every route behind that credential, so widen
        // the exclusion set: offering a sibling model on the same broken
        // credential would fail identically.
        let failover_decision = jcode_provider_core::classify_failover_error_message(error);
        let options = crate::provider::FallbackPickOptions {
            credential_failure: crate::provider::error_looks_like_credential_failure(error)
                || failover_decision.should_mark_provider_unavailable()
                || crate::provider::parse_failover_prompt_message(error).is_some(),
        };
        if !self.is_remote
            && failover_decision.should_mark_provider_unavailable()
            && let Some((method, label)) = current_api_method.split_once('@')
        {
            let provider = match method {
                "claude-oauth" => Some("claude"),
                "openai-oauth" => Some("openai"),
                _ => None,
            };
            if let Some(provider) = provider {
                crate::provider::models::record_provider_unavailable_for_account_label(
                    provider,
                    Some(label),
                    error,
                );
            }
        }
        if routes.is_empty() {
            return false;
        }
        let pool = crate::auth::account_pool::AccountPool::load();
        let source_identity = Self::fallback_pool_identity(&current_api_method);
        let source_is_pooled = match source_identity.as_ref() {
            Some((key, api_key)) => {
                !pool.excluded.contains(key)
                    && pool.is_member(
                        key,
                        crate::auth::account_pool::default_member(key, *api_key),
                    )
            }
            // An unlabeled OAuth source (e.g. a remote session that does not
            // know the account label) is still a subscription login, which is
            // pooled by default. Treat it as pooled unless every account of
            // that provider was excluded, so failover is automatic instead of
            // falling back to a manual Ctrl+Y offer.
            None => {
                let prefix = match current_api_method.as_str() {
                    "claude-oauth" => Some("claude:"),
                    "openai-oauth" => Some("openai:"),
                    _ => None,
                };
                prefix.is_some_and(|prefix| {
                    let excluded_all = !pool.excluded.is_empty()
                        && pool.excluded.iter().any(|key| key.starts_with(prefix))
                        && !pool.members.iter().any(|key| key.starts_with(prefix));
                    !excluded_all
                })
            }
        };
        let automatic = crate::provider::parse_failover_prompt_message(error).is_none()
            && (options.credential_failure || failover_decision.should_failover())
            && source_is_pooled
            && !crate::tui::is_ssh_remote()
            && crate::config::Config::load()
                .provider
                .cross_provider_failover
                == crate::config::CrossProviderFailoverMode::Countdown;
        // One shared ranking for both the automatic and manual paths: pool
        // order, then newest generation, then affinity.
        let ranked = jcode_provider_core::rank_fallback_routes(
            &routes,
            &current_model,
            &current_provider,
            &current_api_method,
            options,
            &pool.ordered_members(),
        );
        if automatic {
            // Never let an excluded preferred route hide an approved later one.
            let approved = ranked.iter().map(|index| &routes[*index]).find(|route| {
                let selection = crate::provider::RouteSelection::from_model_route(route);
                crate::provider::MultiProvider::is_failover_route_pool_approved(
                    &selection.routed_model_spec(),
                ) && Self::fallback_pool_identity(&route.api_method) != source_identity
            });
            if let Some(route) = approved {
                let target_route =
                    crate::provider::RouteSelection::from_model_route(route).routed_model_spec();
                self.handle_provider_failover_prompt_with_payload(
                    crate::provider::ProviderFailoverPrompt {
                        from_provider: current_provider.clone(),
                        from_label: current_provider,
                        to_provider: route.provider.clone(),
                        to_label: Self::describe_route(route),
                        target_route: Some(target_route),
                        reason: error.to_string(),
                        estimated_input_chars: 0,
                        estimated_input_tokens: 0,
                    },
                    remote_resend,
                );
                return true;
            }
        }
        // No approved automatic target: preserve unrestricted manual choice,
        // still ordered by the shared ranking.
        let Some(&index) = ranked.first() else {
            return false;
        };
        let route = routes[index].clone();
        let target_label = Self::describe_route(&route);
        let from_method = crate::provider::ModelRouteApiMethod::parse(&current_api_method);
        let from_label = if current_api_method.is_empty() {
            current_provider.clone()
        } else {
            format!("{} via {}", current_provider, from_method.display_label())
        };

        let remote_resend = if self.is_remote { remote_resend } else { None };

        // The raw error was already shown by the caller just above this offer,
        // so only name the failed route here instead of echoing the error text
        // a second time.
        let key_label = crate::tui::keybind::fallback_switch_key_label();
        self.push_display_message(DisplayMessage::system(format!(
            "↪ Press {} to switch to {} and resend.",
            key_label, target_label,
        )));
        self.set_status_notice(format!("Press {} to switch to {}", key_label, route.model));
        self.pending_fallback_offer = Some(super::super::PendingFallbackOffer {
            selection: crate::provider::RouteSelection::from_model_route(&route),
            target_label,
            from_label,
            remote_resend,
        });
        true
    }
}

impl App {
    pub(in super::super) fn handle_provider_failover_prompt_with_payload(
        &mut self,
        prompt: crate::provider::ProviderFailoverPrompt,
        remote_resend: Option<super::super::FallbackResendPayload>,
    ) {
        // Duplicate provider prompts must not replace the held payload, restart
        // a countdown, or dispatch another switch before acknowledgement.
        if self.pending_provider_failover.is_some()
            || self.pending_automatic_failover_route.is_some()
            || self.pending_automatic_failover_request_id.is_some()
            || self.automatic_failover_stopped
        {
            return;
        }
        super::super::commands::disable_auto_poke(self);
        self.overnight_auto_poke = None;
        self.clear_pending_remote_retry();
        self.pending_provider_failover = None;
        self.pending_automatic_failover_route = None;
        self.clear_pending_fallback_offer();
        let reason = Self::compact_provider_failure(&prompt.reason);
        // The caller often printed the provider error just above. Do not
        // repeat the same cause on the next line.
        let cause_already_shown = self
            .display_messages
            .last()
            .is_some_and(|message| message.role == "error" && message.content.contains(&reason));
        let manual_message = if cause_already_shown {
            format!("Use /model to switch to {} and resend.", prompt.to_label)
        } else {
            format!(
                "{} unavailable: {}\nUse /model to switch to {} and resend.",
                prompt.from_label, reason, prompt.to_label,
            )
        };

        let config = crate::config::Config::load().provider;
        if remote_resend
            .as_ref()
            .is_some_and(|payload| payload.retry_attempts >= Self::AUTO_RETRY_MAX_ATTEMPTS)
            && config.cross_provider_failover == crate::config::CrossProviderFailoverMode::Countdown
        {
            self.automatic_failover_stopped = true;
            if let Some(payload) = remote_resend {
                self.restore_failover_payload(payload);
            }
            self.push_display_message(DisplayMessage::system(format!(
                "Stopped automatic failover after {} retries. Use /model to choose a working account and resend manually.",
                Self::AUTO_RETRY_MAX_ATTEMPTS
            )));
            self.set_status_notice("Paused: automatic failover retry limit reached");
            return;
        }
        match config.cross_provider_failover {
            crate::config::CrossProviderFailoverMode::Manual => {
                if let Some(payload) = remote_resend {
                    self.restore_failover_payload(payload);
                }
                self.push_display_message(DisplayMessage::system(manual_message));
                self.set_status_notice(format!(
                    "{} unavailable; switch manually if desired",
                    prompt.from_label
                ));
            }
            crate::config::CrossProviderFailoverMode::Countdown
                if prompt.target_route.as_deref().is_some_and(|route| {
                    self.is_remote
                        || crate::provider::MultiProvider::is_failover_route_pool_approved(route)
                }) && (!self.is_remote || remote_resend.is_some()) =>
            {
                let Some(target) = prompt.target_route.as_ref() else {
                    return;
                };
                if self.automatic_failover_targets.contains(target)
                    || self.automatic_failover_targets.len()
                        >= usize::from(Self::AUTO_RETRY_MAX_ATTEMPTS)
                {
                    self.automatic_failover_stopped = true;
                    if let Some(payload) = remote_resend {
                        self.restore_failover_payload(payload);
                    }
                    self.pending_turn = false;
                    self.push_display_message(DisplayMessage::system(
                        "Stopped automatic failover: no working route found. Use /model to choose an account and resend manually.".to_string(),
                    ));
                    self.set_status_notice("Paused: automatic failover stopped");
                    return;
                }
                // Only the first switch gets the fast path. An exhausted
                // fallback must not create an instantaneous account loop.
                let immediate = config.exhausted_usage_immediate_failover
                    && self.automatic_failover_targets.is_empty();
                self.automatic_failover_targets.push(target.clone());
                self.pending_provider_failover = Some(super::super::PendingProviderFailover {
                    prompt: prompt.clone(),
                    deadline: Instant::now() + Duration::from_secs(if immediate { 0 } else { 3 }),
                    remote_resend,
                });
                // One line for the cause, one line for the switch. Nothing else.
                // Callers that already printed the provider error skip the cause.
                if !self
                    .display_messages
                    .last()
                    .is_some_and(|message| message.role == "error")
                {
                    self.push_display_message(DisplayMessage::error(format!(
                        "{}: {}",
                        prompt.from_label, reason
                    )));
                }
                if immediate {
                    // Reuse the normal switch path, including target/pool revalidation
                    // and remote route acknowledgement before resending the payload.
                    self.maybe_progress_provider_failover_countdown();
                    if self.pending_automatic_failover_route.is_some() {
                        self.push_display_message(DisplayMessage::system(
                            Self::failover_switching_message(&prompt.to_label),
                        ));
                    }
                    return;
                }
                self.push_display_message(DisplayMessage::system(
                    Self::failover_countdown_message(&prompt.to_label),
                ));
                self.set_status_notice(format!(
                    "Provider auto-switch → {} in 3s (Esc to cancel)",
                    prompt.to_label
                ));
            }
            _ => {
                if let Some(payload) = remote_resend {
                    self.restore_failover_payload(payload);
                }
                self.push_display_message(DisplayMessage::system(manual_message));
                self.set_status_notice(format!(
                    "{} unavailable; manual switch suggested",
                    prompt.from_label
                ));
            }
        }
    }
}

impl App {
    /// Attempt automatic compaction and retry when context limit is exceeded.
    /// Returns true if the retry succeeded.
    pub(in super::super) async fn try_auto_compact_and_retry(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> bool {
        if self.is_remote || !self.provider.supports_compaction() {
            return false;
        }

        self.push_display_message(DisplayMessage::system(
            "⚠️ Context limit exceeded - auto-compacting and retrying...".to_string(),
        ));

        // Force the compaction manager to think we're at the limit
        let compaction = self.registry.compaction();
        let compact_started = match compaction.try_write() {
            Ok(mut manager) => {
                let mut provider_messages = self.materialized_provider_messages();
                manager.update_observed_input_tokens(self.context_limit);
                let usage = manager.context_usage_with(&provider_messages);
                if usage > 1.5 {
                    let recovery = manager.recover_within_budget(&mut provider_messages);
                    if recovery.did_anything() {
                        self.messages = provider_messages;
                        self.sync_session_compaction_state_from_manager(&manager);
                        drop(manager);
                        self.reset_state_for_compaction_retry();

                        self.push_display_message(DisplayMessage::system(format!(
                            "{} Retrying...",
                            recovery.summary_line(usage)
                        )));
                        return self.run_compaction_retry_turn(terminal, event_stream).await;
                    }
                    false
                } else {
                    match manager.force_compact_with(&provider_messages, self.provider.clone()) {
                        Ok(()) => true,
                        Err(_) => match manager.hard_compact_with(&provider_messages) {
                            Ok(_) => {
                                self.sync_session_compaction_state_from_manager(&manager);
                                drop(manager);
                                self.reset_state_for_compaction_retry();

                                self.push_display_message(DisplayMessage::system(
                                    "✓ Context compacted (emergency). Retrying...".to_string(),
                                ));
                                return self
                                    .run_compaction_retry_turn(terminal, event_stream)
                                    .await;
                            }
                            Err(_) => false,
                        },
                    }
                }
            }
            Err(_) => false,
        };

        if !compact_started {
            return false;
        }

        // Wait for compaction to finish (up to 60s), reacting to Bus event
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        self.status = ProcessingStatus::RunningTool("compacting context...".to_string());
        let mut bus_rx = Bus::global().subscribe();

        loop {
            if std::time::Instant::now() >= deadline {
                self.push_display_message(DisplayMessage::error(
                    "Auto-compaction timed out.".to_string(),
                ));
                return false;
            }

            // Redraw UI while we wait
            let _ = terminal.draw(|frame| crate::tui::ui::draw(frame, self));

            let compaction = self.registry.compaction();
            let done = if let Ok(mut manager) = compaction.try_write() {
                let provider_messages = self.materialized_provider_messages();
                if let Some(event) = manager.poll_compaction_event_with(&provider_messages) {
                    self.sync_session_compaction_state_from_manager(&manager);
                    self.handle_compaction_event(event);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if done {
                break;
            }

            // Wait for Bus notification or timeout (instead of sleep-polling)
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::select! {
                _ = bus_rx.recv() => {}
                _ = timeout => {}
            }
        }

        self.push_display_message(DisplayMessage::system(
            "✓ Context compacted. Retrying...".to_string(),
        ));

        self.reset_state_for_compaction_retry();

        // Retry the turn
        self.run_compaction_retry_turn(terminal, event_stream).await
    }
}
