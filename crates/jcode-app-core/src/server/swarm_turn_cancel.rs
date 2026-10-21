//! Explicit teardown cancels continuation ownership before dropping a runtime.
use super::*;

pub(in crate::server) async fn cancel_session_turn(session_id: &str) {
    cancel_session_turn_with(session_id, |_| async {}).await;
}

pub(in crate::server) async fn cancel_session_turn_with<F, Fut>(session_id: &str, stopped: F)
where
    F: FnOnce(bool) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if let Some(publication) = turn_publication_for(session_id).await {
        let mut state = publication.state.lock().await;
        let owns_turn = *state == TurnPublicationState::Running;
        let signals = crate::turn_cancel_registry::active_turn_signals(session_id);
        let had_turn = owns_turn
            || !signals.is_empty()
            || super::super::usage_limit_resume::has_pending_resume(session_id);
        if owns_turn {
            *state = TurnPublicationState::Cancelled;
            publication.cancelled.cancel();
        }
        super::super::usage_limit_resume::cancel_pending_resume(session_id);
        for signal in signals {
            signal.fire();
        }
        stopped(had_turn).await;
    } else {
        let signals = crate::turn_cancel_registry::active_turn_signals(session_id);
        let had_turn =
            !signals.is_empty() || super::super::usage_limit_resume::has_pending_resume(session_id);
        super::super::usage_limit_resume::cancel_pending_resume(session_id);
        for signal in signals {
            signal.fire();
        }
        stopped(had_turn).await;
    }
}
