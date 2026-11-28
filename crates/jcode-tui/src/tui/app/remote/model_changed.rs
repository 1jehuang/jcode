use super::*;

pub(super) fn handle_model_changed(app: &mut App, event: ServerEvent) -> bool {
    let ServerEvent::ModelChanged {
        id,
        model,
        provider_name,
        error,
        resolved_credential,
        provider_oauth_email,
        provider_oauth_account_label,
        reasoning_effort,
        context_window,
        ..
    } = event
    else {
        return false;
    };
    // A resume/catalog acknowledgement can arrive before the staged
    // route request has even been sent. It cannot authorize a resend.
    if app.pending_automatic_failover_request_id.is_none()
        && (app.pending_automatic_failover_route.is_some()
            || (app.pending_route_selection.is_some() && app.pending_fallback_resend.is_some()))
    {
        return false;
    }
    if app
        .pending_automatic_failover_request_id
        .is_some_and(|expected| expected != id)
    {
        return false;
    }
    app.pending_automatic_failover_request_id = None;
    app.pending_automatic_failover_started = None;
    app.remote_model_switch_in_flight = false;
    let automatic_route = app.pending_automatic_failover_route.take();
    let error = error.or_else(|| {
                let selection = automatic_route.as_ref()?;
                use jcode_provider_core::{ResolvedCredential, RuntimeKey};
                let expected_credential = match selection.runtime_key {
                    RuntimeKey::ClaudeOAuth | RuntimeKey::OpenAIOAuth => {
                        Some(ResolvedCredential::Oauth)
                    }
                    RuntimeKey::AnthropicApiKey | RuntimeKey::OpenAIApiKey => {
                        Some(ResolvedCredential::ApiKey)
                    }
                    _ => None,
                };
                let expected_provider =
                    jcode_provider_core::parse_provider_hint(&selection.provider_label);
                let actual_provider = provider_name
                    .as_deref()
                    .and_then(jcode_provider_core::parse_provider_hint);
                let matches = selection.model == model
                    && selection.account_label().is_none_or(|label| {
                        provider_oauth_account_label.as_deref() == Some(label)
                    })
                    && expected_credential.is_none_or(|credential| {
                        resolved_credential == Some(credential)
                    })
                    && expected_provider.is_none_or(|provider| actual_provider == Some(provider));
                (!matches).then(|| {
                    "The server did not confirm the requested failover model/account. Automatic resend stopped. Use /model to choose a route and resend.".to_string()
                })
            });
    if let Some(err) = app.remember_claude_cli_route_error(id, error) {
        let account_switch_failed = app.pending_account_switch_target.take().is_some();
        if let Some(prepared) = app.pending_prompt_after_model_switch.take() {
            super::input_dispatch::restore_prepared_remote_input(app, prepared);
        }
        // A fallback-offer resend cannot go out on the failed switch;
        // drop it and put the prompt back in the input box instead.
        if let Some(payload) = app.pending_fallback_resend.take() {
            app.restore_failover_payload(payload);
        }
        app.push_display_message(DisplayMessage::error(
            crate::tui::app::model_context::model_switch_failure_message(&err, true),
        ));
        app.set_status_notice(if account_switch_failed {
            "Account switch failed"
        } else {
            "Model switch failed"
        });
    } else {
        // ModelChanged predates capability metadata. Fetch the typed
        // History catalog after a successful model/account switch.
        app.model_catalog_after_history = true;
        let account_actually_changed = !app.remote_session_facts_provisional
            && app.remote_provider_model.is_some()
            && (app.remote_provider_oauth_account_label != provider_oauth_account_label
                || app.remote_provider_oauth_email != provider_oauth_email
                || app.remote_resolved_credential != resolved_credential);
        if app.remote_provider_model.as_deref() != Some(model.as_str())
            || app.remote_provider_oauth_account_label != provider_oauth_account_label
            || app.remote_provider_oauth_email != provider_oauth_email
            || app.remote_resolved_credential != resolved_credential
            || provider_name
                .as_ref()
                .is_some_and(|name| app.remote_provider_name.as_ref() != Some(name))
        {
            app.remote_available_service_tiers = None;
            app.usage_reset.openai_quota_rejected = false;
        }
        app.remote_provider_oauth_email = provider_oauth_email;
        app.remote_provider_oauth_account_label = provider_oauth_account_label;
        // The server also re-sends ModelChanged on resume so the client
        // learns the server-resolved context window. That is not a
        // user-visible switch, so only announce an actual model change.
        // A first-frame hint is not a model the user switched away from.
        let had_provisional_facts = std::mem::take(&mut app.remote_session_facts_provisional);
        let model_actually_changed =
            !had_provisional_facts && app.remote_provider_model.as_deref() != Some(model.as_str());
        app.update_context_limit_for_model(&model, context_window);
        app.remote_provider_model = Some(model.clone());
        app.clear_remote_startup_phase();
        if let Some(ref pname) = provider_name {
            app.remote_provider_name = Some(pname.clone());
        }
        let automatic_failover = automatic_route.is_some();
        if let Some(selection) = automatic_route {
            app.session.route_api_method = Some(selection.api_method);
        }
        let acknowledged_provider = app
            .remote_provider_name
            .as_deref()
            .and_then(jcode_provider_core::parse_provider_hint);
        let account_switch_acknowledged = matches!(
            app.pending_account_switch_target.as_ref(),
            Some(crate::tui::AccountPickerAction::Switch { provider_id, label })
                if app.remote_provider_oauth_account_label.as_ref() == Some(label)
                    && acknowledged_provider.is_some()
                    && jcode_provider_core::parse_provider_hint(provider_id)
                        == acknowledged_provider
        );
        if account_switch_acknowledged {
            app.pending_account_switch_target = None;
        }
        // Several cycles may be queued while the daemon is busy. An
        // earlier acknowledgement updates confirmed state, but must
        // not discard the newer target used to anchor the next cycle.
        // Persist the daemon's label, never the optimistic target.
        if resolved_credential == Some(jcode_provider_core::ResolvedCredential::Oauth) {
            let method = match acknowledged_provider {
                Some(jcode_provider_core::ActiveProvider::Claude) => Some("claude-oauth"),
                Some(jcode_provider_core::ActiveProvider::OpenAI) => Some("openai-oauth"),
                _ => None,
            };
            if let Some(method) = method {
                let route = app
                    .remote_provider_oauth_account_label
                    .as_deref()
                    .map(|label| format!("{method}@{label}"))
                    .filter(|route| jcode_provider_core::oauth_account_label(route).is_some())
                    .unwrap_or_else(|| method.to_string());
                app.session.route_api_method = Some(route);
            }
        }
        // Always replace: a switch to a provider with no OAuth/API
        // distinction must clear the previous route's credential too.
        app.remote_resolved_credential = resolved_credential;
        // Always replace: the server reports the effort the new model
        // runs with (`None` = cleared), so the chip must not keep the
        // previous model's level.
        app.remote_reasoning_effort = reasoning_effort;
        app.invalidate_model_picker_cache();
        app.persist_remote_header_hint();
        if model_actually_changed && !app.auth_catalog_refresh_pending {
            let is_oauth = app.remote_resolved_credential
                == Some(jcode_provider_core::ResolvedCredential::Oauth);
            let suffix = crate::tui::app::helpers::switched_model_account_suffix(
                app.remote_resolved_credential
                    .map(|credential| credential.auth_method_label()),
                app.remote_provider_oauth_email
                    .as_ref()
                    .filter(|_| is_oauth)
                    .map(|email| email.as_str()),
            );
            // One line for an automatic failover: the confirmed route
            // replaces the in-flight "Switching to" line and absorbs
            // a usage-limit error as its cause.
            let message = if automatic_failover {
                app.confirmed_failover_message(&model, &suffix)
            } else {
                format!("✓ Switched to model: {}{}", model, suffix)
            };
            app.push_display_message(DisplayMessage::system(message));
        }
        if model_actually_changed {
            app.set_status_notice(format!("Model → {}", model));
        } else if account_actually_changed || account_switch_acknowledged {
            let label = crate::tui::TuiState::startup_oauth_account_label(app);
            app.set_status_notice(match label {
                Some(label) => format!("Account → {}", label),
                None => "Account updated".to_string(),
            });
        }
    }
    // Same-model account acknowledgements change header identity too.
    // Clear cached header preparation and request a frame even when no
    // transcript announcement was added (including switch failures).
    crate::tui::ui::prepare::invalidate_header_prep_cache();
    app.note_claude_cli_model_ack(id)
}
