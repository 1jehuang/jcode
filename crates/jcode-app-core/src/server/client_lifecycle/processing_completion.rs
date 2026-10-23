use super::*;

/// Retire finished turns before interpreting a request against client-local state.
/// The task publishes completion before its terminal event, and never waits on
/// this actor. Joining it also closes the cross-thread gap between those sends:
/// a new turn cannot start before the old terminal is on the ordered stream.
pub(super) async fn reconcile_processing_completions(
    mut completion: Option<ProcessingCompletion>,
    receiver: &mut mpsc::UnboundedReceiver<ProcessingCompletion>,
    state: &mut ProcessingState<'_>,
    connection_id: &str,
    connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm: &SwarmStatusRefs<'_>,
) {
    while let Some((done_id, result, completion_report)) =
        completion.take().or_else(|| match receiver.try_recv() {
            Ok(completion) => Some(completion),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                crate::logging::warn("Turn completion channel disconnected");
                None
            }
        })
    {
        if Some(done_id) != *state.message_id {
            crate::logging::warn(&format!(
                "Done event id={} doesn't match processing_message_id={:?}, dropping",
                done_id, state.message_id
            ));
            continue;
        }
        if let Some(task) = state.task.take()
            && let Err(error) = task.await
        {
            crate::logging::warn(&format!("Completed turn task join failed: {error}"));
        }
        crate::logging::info(&format!(
            "Processing done for message id={}, result={}",
            done_id,
            if result.is_ok() { "ok" } else { "err" }
        ));
        *state.message_id = None;
        *state.client_is_processing = false;
        {
            let mut connections = connections.write().await;
            if let Some(info) = connections.get_mut(connection_id) {
                info.is_processing = false;
                info.current_tool_name = None;
            }
        }
        let done_session = state.session_id.take();
        record_processing_completion(done_session.as_deref(), result, completion_report, swarm)
            .await;
    }
}

pub(super) async fn record_processing_completion(
    done_session: Option<&str>,
    result: Result<()>,
    completion_report: Option<String>,
    swarm: &SwarmStatusRefs<'_>,
) {
    match result {
        Ok(()) => {
            if let Some(session_id) = done_session {
                update_member_status_with_report(
                    session_id,
                    "ready",
                    None,
                    completion_report,
                    swarm.members,
                    swarm.swarms_by_id,
                    Some(swarm.event_history),
                    Some(swarm.event_counter),
                    Some(swarm.event_tx),
                )
                .await;
            }
        }
        Err(e) => {
            if let Some(session_id) = done_session {
                update_member_status(
                    session_id,
                    "failed",
                    Some(truncate_detail(&e.to_string(), 120)),
                    swarm.members,
                    swarm.swarms_by_id,
                    Some(swarm.event_history),
                    Some(swarm.event_counter),
                    Some(swarm.event_tx),
                )
                .await;
            }
            let retry_after_secs = e
                .downcast_ref::<StreamError>()
                .and_then(|se| se.retry_after_secs);
            if retry_after_secs.is_some() {
                crate::telemetry::record_error(crate::telemetry::ErrorCategory::RateLimited);
            } else {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                if lower.contains("timeout") {
                    crate::telemetry::record_error(
                        crate::telemetry::ErrorCategory::ProviderTimeout,
                    );
                } else if crate::provider::error_looks_like_credential_failure(&msg)
                    || lower.contains("403 forbidden")
                {
                    // Use the shared credential-failure classifier instead of a
                    // bare `contains("auth")`: that substring also matched
                    // unrelated errors (e.g. any message mentioning "author" or
                    // OAuth flow noise) and inflated the auth_failed telemetry
                    // counter.
                    crate::telemetry::record_error(crate::telemetry::ErrorCategory::AuthFailed);
                }
            }
        }
    }
}
