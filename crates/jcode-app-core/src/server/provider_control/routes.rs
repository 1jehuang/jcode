use super::*;

fn verify_origin(
    agent: &Agent,
    verification: &jcode_provider_core::AuthRouteVerification,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        agent.provider_model() == verification.previous_model
            && jcode_provider_core::model_route_provider_labels_match(
                &agent.provider_name(),
                &verification.previous_provider
            )
            && agent.provider_handle().serving_oauth_account_label()
                == verification.previous_serving_account_label
            && match verification.previous_credential {
                Some(expected) => agent.active_resolved_credential() == Some(expected),
                None =>
                    agent.active_resolved_credential()
                        != Some(jcode_provider_core::ResolvedCredential::ApiKey),
            },
        "Model or account selection changed before the completed Claude login could be applied"
    );
    Ok(())
}

#[cfg(test)]
pub(in crate::server) async fn handle_set_route(
    id: u64,
    selection: RouteSelection,
    automatic_failover: bool,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    handle_set_route_verified(
        id,
        selection,
        automatic_failover,
        None,
        agent,
        client_event_tx,
    )
    .await;
}

fn apply_set_route(
    id: u64,
    selection: crate::provider::RouteSelection,
    automatic_failover: bool,
    verification: Option<jcode_provider_core::AuthRouteVerification>,
    agent: &mut Agent,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::logging::event_info(
        "server_set_route_request",
        vec![
            ("id", id.to_string()),
            ("requested_model", selection.model.clone()),
            ("requested_provider", selection.provider_label.clone()),
            ("requested_api_method", selection.api_method.clone()),
            ("current_model", agent.provider_model()),
            ("current_provider", agent.provider_name()),
        ],
    );

    let current = agent.provider_model();
    let result = {
        let ineligible = if !automatic_failover {
            None
        } else if crate::config::Config::load()
            .provider
            .cross_provider_failover
            != crate::config::CrossProviderFailoverMode::Countdown
        {
            Some("countdown auto-switch is disabled")
        } else if !automatic_failover_source_is_pool_member(agent) {
            Some("source account is not in the auto-switch group")
        } else if !crate::provider::MultiProvider::is_failover_route_pool_approved(
            &selection.routed_model_spec(),
        ) {
            Some("target account is not eligible in the auto-switch group")
        } else {
            None
        };
        let result = if let Some(why) = ineligible {
            Err(anyhow::anyhow!("Auto-switch canceled: {why}."))
        } else if let Some(verification) = &verification {
            crate::auth::claude_cli_completion::with_verified_receipt(
                &verification.completion_path,
                &verification.attempt,
                &verification.fingerprint,
                || {
                    verify_origin(agent, verification)?;
                    agent.set_route_selection_verified(&selection, &verification.fingerprint)
                },
            )
        } else {
            agent.set_route_selection(&selection)
        };
        if result.is_ok() {
            agent.reset_provider_session();
        }
        result.map(|_| {
            (
                agent.provider_model(),
                agent.provider_name(),
                agent.active_resolved_credential(),
                agent.provider_reasoning_effort(),
            )
        })
    };
    if verification.is_some()
        && let Err(error) = client_event_tx.send(ServerEvent::AuthRouteVerified {
            id,
            verified: result.is_ok(),
        })
    {
        crate::logging::debug(&format!(
            "Client disconnected before sign-in acknowledgement: {error}"
        ));
    }
    send_model_changed_result(id, result, current, agent, client_event_tx);
}

pub(in crate::server) async fn handle_set_route_verified(
    id: u64,
    selection: crate::provider::RouteSelection,
    automatic_failover: bool,
    verification: Option<jcode_provider_core::AuthRouteVerification>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Ok(mut agent_guard) = agent.try_lock() {
        apply_set_route(
            id,
            selection,
            automatic_failover,
            verification,
            &mut agent_guard,
            client_event_tx,
        );
    } else {
        spawn_deferred_agent_mutation(
            "set_route",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            move |agent_guard, client_event_tx| {
                apply_set_route(
                    id,
                    selection,
                    automatic_failover,
                    verification,
                    agent_guard,
                    client_event_tx,
                );
            },
        );
    }
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
