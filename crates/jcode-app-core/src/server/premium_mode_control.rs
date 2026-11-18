//! Premium mode mutation and deferred client acknowledgements.

use super::*;

pub(super) fn premium_mode_label(mode: crate::provider::copilot::PremiumMode) -> &'static str {
    use crate::provider::copilot::PremiumMode;
    match mode {
        PremiumMode::Zero => "zero premium requests",
        PremiumMode::OnePerSession => "one premium per session",
        PremiumMode::Normal => "normal",
    }
}

pub(super) fn apply_set_premium_mode(
    id: u64,
    mode: u8,
    premium_mode: crate::provider::copilot::PremiumMode,
    agent: &Agent,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    agent.set_premium_mode(premium_mode);
    crate::logging::info(&format!(
        "Server: premium mode set to {} ({})",
        mode,
        premium_mode_label(premium_mode)
    ));
    if let Err(error) = client_event_tx.send(ServerEvent::Ack { id }) {
        crate::logging::debug(&format!(
            "Model change acknowledgement client disconnected: {error}"
        ));
    }
}

pub(in crate::server) async fn handle_set_premium_mode(
    id: u64,
    mode: u8,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    use crate::provider::copilot::PremiumMode;

    let premium_mode = match mode {
        2 => PremiumMode::Zero,
        1 => PremiumMode::OnePerSession,
        _ => PremiumMode::Normal,
    };
    if let Ok(agent_guard) = agent.try_lock() {
        apply_set_premium_mode(id, mode, premium_mode, &agent_guard, client_event_tx);
    } else {
        spawn_deferred_agent_mutation(
            "set_premium_mode",
            id,
            Arc::clone(agent),
            client_event_tx.clone(),
            move |agent_guard, client_event_tx| {
                apply_set_premium_mode(id, mode, premium_mode, agent_guard, client_event_tx);
            },
        );
    }
}
