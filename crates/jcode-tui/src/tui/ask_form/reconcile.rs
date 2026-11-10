//! Lifecycle and reconnect reconciliation for the `ask_user` form.
//!
//! The daemon's form broker is in memory, so a reload drops pending forms
//! without a resolution. A reconnect replays what is still pending and then
//! emits `ElicitationReplayComplete` (added to the protocol in lane C). Between
//! those two points forms are frozen drafts; replay restores them only against
//! an identical schema.

use crate::protocol::ServerEvent;
use crate::protocol::elicitation::ElicitationResolution;

use super::ViewContext;
use super::schema;
use super::state::{Form, ResponseOwner, State, SubmitState};

pub(super) struct EventResult {
    pub consumed: bool,
    pub notice: Option<String>,
}

impl EventResult {
    fn ignored() -> Self {
        Self {
            consumed: false,
            notice: None,
        }
    }

    fn consumed() -> Self {
        Self {
            consumed: true,
            notice: None,
        }
    }

    fn notified(notice: impl Into<String>) -> Self {
        Self {
            consumed: true,
            notice: Some(notice.into()),
        }
    }
}

pub(super) fn handle_event(
    state: &mut State,
    context: &ViewContext,
    event: &ServerEvent,
) -> EventResult {
    // Never answer while replaying recorded history.
    if context.replay {
        return EventResult::ignored();
    }

    match event {
        ServerEvent::ElicitationRequest(request) => {
            if !adopt_session(state, Some(&request.session_id), context) {
                return EventResult::ignored();
            }
            state.attached = true;

            // v1 shows tagged `ask_user` forms only. Applet and other forms
            // are ignored without a response so another attached client can
            // still answer them.
            if !schema::is_tagged(&request.requested_schema.0) {
                return EventResult::consumed();
            }

            let generation = state.generation;
            if let Some(index) = state.find_form(&request.session_id, &request.elicitation_id) {
                if state.forms[index].schema == request.requested_schema {
                    // Replayed identical form: restore the draft untouched.
                    state.forms[index].message = request.message.clone();
                    state.forms[index].seen_generation = Some(generation);
                } else {
                    let replacement = Form::new(request, generation);
                    match replacement {
                        Some(form) => state.forms[index] = form,
                        None => {
                            state.forms.remove(index);
                            if state.active >= state.forms.len() {
                                state.active = state.forms.len().saturating_sub(1);
                            }
                        }
                    }
                }
            } else if let Some(form) = Form::new(request, generation) {
                if state.forms.is_empty() {
                    state.active = 0;
                }
                state.forms.push(form);
            }
            EventResult::consumed()
        }
        ServerEvent::ElicitationResolved {
            session_id,
            elicitation_id,
            outcome,
        } => {
            if state.session.as_deref() != Some(session_id) {
                return EventResult::ignored();
            }
            let Some(form) = state.remove_form(session_id, elicitation_id) else {
                return EventResult::ignored();
            };
            let notice = match outcome {
                ElicitationResolution::Accepted => {
                    if form.submit == SubmitState::Submitting {
                        "Answer submitted.".to_string()
                    } else {
                        "Question form answered elsewhere.".to_string()
                    }
                }
                ElicitationResolution::Declined => "Question form declined.".to_string(),
                ElicitationResolution::Cancelled => "Question form cancelled.".to_string(),
                ElicitationResolution::Expired => "Question form expired.".to_string(),
            };
            EventResult::notified(notice)
        }
        ServerEvent::Done { id } => {
            if state.responses.remove(id).is_some() {
                EventResult::consumed()
            } else {
                EventResult::ignored()
            }
        }
        ServerEvent::Error { id, message, .. } => {
            let Some(owner) = state.responses.remove(id) else {
                return EventResult::ignored();
            };
            let Some(index) = state.find_form(&owner.session, &owner.elicitation_id) else {
                // Resolution wins: a resolved form is never resurrected.
                return EventResult::consumed();
            };
            if state.forms[index].submit != SubmitState::Submitting {
                return EventResult::consumed();
            }
            state.forms[index].restore_editing_after_rejection(message.clone());
            EventResult::notified(format!("The answer was rejected: {message}"))
        }
        // Same-session `History` refreshes are not replay completion. Forms,
        // drafts and response ids survive. Everything else is not ours.
        _ => EventResult::ignored(),
    }
}

pub(super) fn record_response_id(state: &mut State, request_id: u64) {
    let Some(target) = state.last_sent.take() else {
        return;
    };
    state.responses.insert(
        request_id,
        ResponseOwner {
            session: target.session,
            elicitation_id: target.elicitation_id,
        },
    );
}

pub(super) fn send_failed(state: &mut State, _context: &ViewContext, message: &str) -> bool {
    let Some(target) = state.last_sent.as_ref() else {
        return false;
    };
    let session = target.session.clone();
    let elicitation_id = target.elicitation_id.clone();
    let Some(index) = state.find_form(&session, &elicitation_id) else {
        // Resolution wins over a later local failure.
        state.last_sent = None;
        return false;
    };
    if state.forms[index].submit != SubmitState::Submitting {
        state.last_sent = None;
        return false;
    }
    state.forms[index].restore_editing_after_rejection(message.to_string());
    state.last_sent = None;
    true
}

pub(super) fn connection_replaced(state: &mut State, app_session: Option<&str>) {
    state.generation = state.generation.wrapping_add(1).max(1);
    state.attached = false;
    state.responses.clear();
    state.last_sent = None;

    if let Some(session) = app_session
        && state.session.as_deref() != Some(session)
    {
        // A session change clears the old session's drafts immediately.
        state.clear_forms();
        state.session = Some(session.to_string());
        return;
    }

    // Same session: freeze drafts and wait for replay.
    for form in &mut state.forms {
        form.seen_generation = None;
    }
}

pub(super) fn on_replay_complete(
    state: &mut State,
    context: &ViewContext,
    session_id: &str,
) -> Option<String> {
    if state.session.as_deref() != Some(session_id)
        && !adopt_session(state, Some(session_id), context)
    {
        return None;
    }
    state.attached = true;
    let dropped = state.discard_unreplayed();
    if dropped > 0 {
        Some(format!(
            "{dropped} question form(s) were lost while reconnecting."
        ))
    } else {
        None
    }
}

/// Adopt a session that the attachment reports before `History` has updated
/// the app's current session. Returns false for an unrelated session.
fn adopt_session(state: &mut State, event_session: Option<&str>, context: &ViewContext) -> bool {
    let Some(event_session) = event_session else {
        return false;
    };
    if state.session.as_deref() == Some(event_session) {
        return true;
    }
    let state_is_current = state.session.as_deref() == context.session.as_deref();
    let event_is_current = context.session.as_deref() == Some(event_session);
    if state.session.is_none() || context.session.is_none() || state_is_current || event_is_current
    {
        state.clear_forms();
        state.session = Some(event_session.to_string());
        true
    } else {
        false
    }
}
