use super::{
    Agent, Arc, Mutex, ServerEvent, mpsc, parse_account_kind, spawn_deferred_agent_mutation,
};

/// `replace_session_account_identity`: after an OAuth "Replace login", move the
/// sender's session from the old identity to the new one, but only where its
/// pin and/or failover home still equal the captured `{ label, old_identity }`.
/// Sends no refresh: the caller follows with `notify_auth_changed`, which does.
pub(in crate::server) async fn handle_replace_session_account_identity(
    id: u64,
    provider: String,
    label: String,
    old_identity: Option<String>,
    new_identity: Option<String>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    supports_session_accounts: bool,
) {
    let label = label.trim().to_string();
    if label.is_empty() {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Failed to replace account identity: missing account label".to_string(),
            retry_after_secs: None,
            server_resumes: false,
        });
        return;
    }
    let Some(kind) = parse_account_kind(id, &provider, Some(&label), client_event_tx) else {
        return;
    };
    let expected = crate::provider::AccountPin::new(label.clone(), old_identity);
    let replacement = crate::provider::AccountPin::new(label, new_identity);
    let apply = move |agent_guard: &mut Agent,
                      client_event_tx: &mpsc::UnboundedSender<ServerEvent>| {
        // Re-check the saved store at execution time (this closure may run
        // deferred): if another replacement has since won, the new identity is
        // no longer stored and the migration is a no-op.
        let replacement_still_stored =
            crate::session_accounts::pin_for_label(kind, &replacement.label)
                .map(|stored| stored == replacement)
                .unwrap_or(false);
        if !replacement_still_stored {
            let _ = client_event_tx.send(ServerEvent::Done { id });
            return;
        }
        match agent_guard.replace_account_pin_identity(kind, &expected, &replacement) {
            Ok(changed) => {
                if changed && supports_session_accounts {
                    let provider = agent_guard.provider_handle();
                    let _ = client_event_tx.send(
                        crate::session_accounts::account_changed_event_with_pins(
                            provider.as_ref(),
                            agent_guard.account_pins(),
                            kind,
                            Some(format!("{} now uses the replaced login", replacement.label)),
                        ),
                    );
                }
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }
            Err(error) => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Failed to replace {} account identity: {error}",
                        crate::session_accounts::provider_display(kind)
                    ),
                    retry_after_secs: None,
                    server_resumes: false,
                });
            }
        }
    };
    if let Ok(mut agent_guard) = agent.try_lock() {
        apply(&mut agent_guard, client_event_tx);
    } else {
        spawn_deferred_agent_mutation(
            "replace_session_account_identity",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            apply,
        );
    }
}
