//! Model-change snapshots shared by session attachment and native control.

use super::*;

pub(in crate::server) fn model_changed_snapshot(id: u64, agent: &Agent) -> ServerEvent {
    ServerEvent::ModelChanged {
        route_selection: None,
        id,
        model: agent.provider_model(),
        provider_name: Some(agent.provider_name()),
        context_window: Some(agent.provider_context_window() as u64),
        error: None,
        resolved_credential: agent.active_resolved_credential(),
        provider_oauth_email: super::super::provider_oauth_email(agent.provider_handle().as_ref()),
        reasoning_effort: agent.provider_reasoning_effort(),
        provider_oauth_account_label: agent.provider_handle().serving_oauth_account_label(),
    }
}

pub(in crate::server) fn send_model_changed_result(
    id: u64,
    result: anyhow::Result<(
        String,
        String,
        Option<jcode_provider_core::ResolvedCredential>,
        Option<String>,
    )>,
    fallback_model: String,
    agent: &mut Agent,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    // A saved-account route can move this window's account; show it now.
    for notice in agent.take_account_notices() {
        if client_event_tx.send(notice).is_err() {
            return;
        }
    }
    match result {
        Ok((updated, provider_name, resolved_credential, reasoning_effort)) => {
            crate::telemetry::record_model_switch();
            crate::logging::event_info(
                "server_model_changed",
                vec![
                    ("id", id.to_string()),
                    ("model", updated.clone()),
                    ("provider", provider_name.clone()),
                ],
            );
            if client_event_tx
                .send(ServerEvent::ModelChanged {
                    route_selection: None,
                    id,
                    model: updated,
                    provider_name: Some(provider_name),
                    context_window: Some(agent.provider_context_window() as u64),
                    error: None,
                    resolved_credential,
                    provider_oauth_email: super::super::provider_oauth_email(
                        agent.provider_handle().as_ref(),
                    ),
                    provider_oauth_account_label: agent
                        .provider_handle()
                        .serving_oauth_account_label(),
                    reasoning_effort,
                })
                .is_err()
            {
                crate::logging::info("Model change listener disconnected");
            }
        }
        Err(error) => {
            crate::logging::event_error(
                "server_model_change_failed",
                vec![
                    ("id", id.to_string()),
                    ("fallback_model", fallback_model.clone()),
                    ("error", error.to_string()),
                ],
            );
            if client_event_tx
                .send(ServerEvent::ModelChanged {
                    route_selection: None,
                    id,
                    model: fallback_model,
                    provider_name: None,
                    context_window: Some(agent.provider_context_window() as u64),
                    error: Some(error.to_string()),
                    resolved_credential: None,
                    provider_oauth_email: None,
                    provider_oauth_account_label: None,
                    reasoning_effort: None,
                })
                .is_err()
            {
                crate::logging::info("Model change listener disconnected");
            }
        }
    }
}
