use super::*;
use crate::{
    ConfidenceState, DeliveryState, GateObservationKind, IntentUnderstanding, is_auto_poke_message,
};

fn todo(content: &str, status: &str) -> TodoItem {
    TodoItem {
        content: content.into(),
        status: status.into(),
        priority: "high".into(),
        id: content.into(),
        ..Default::default()
    }
}

fn done(content: &str, confidence: ConfidenceState) -> TodoItem {
    TodoItem {
        completion_confidence: Some(confidence),
        ..todo(content, "completed")
    }
}

fn snapshot(todos: Vec<TodoItem>) -> TodoSnapshot {
    TodoSnapshot {
        todos,
        ..Default::default()
    }
}

fn kind(decision: &Decision) -> Option<FollowUpKind> {
    decision.follow_up.as_ref().map(|f| f.kind)
}

#[test]
fn disabled_policy_never_follows_up() {
    let mut policy = FollowUpPolicy::new(false);
    assert_eq!(
        policy.decide(&snapshot(vec![todo("a", "pending")])),
        Decision::default()
    );
    policy.enable();
    assert_eq!(
        kind(&policy.decide(&snapshot(vec![todo("a", "pending")]))),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn pokes_open_work_once_per_distinct_state() {
    let mut policy = FollowUpPolicy::new(true);
    let open = snapshot(vec![todo("a", "pending"), todo("b", "in_progress")]);
    let first = policy.decide(&open);
    let follow_up = first.follow_up.expect("poke");
    assert_eq!(follow_up.kind, FollowUpKind::IncompleteTodos);
    assert!(is_auto_poke_message(&follow_up.message));
    assert!(follow_up.notice.contains("2 incomplete todos"));
    // Unchanged open work means the model is stalled: no loop.
    assert_eq!(policy.decide(&open), Decision::default());
    // Progress re-arms the poke.
    let progressed = snapshot(vec![todo("a", "completed"), todo("b", "in_progress")]);
    assert_eq!(
        kind(&policy.decide(&progressed)),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn no_todos_stays_armed_and_quiet() {
    let mut policy = FollowUpPolicy::new(true);
    assert_eq!(policy.decide(&TodoSnapshot::default()), Decision::default());
    assert!(policy.is_enabled());
    assert_eq!(
        kind(&policy.decide(&snapshot(vec![todo("a", "pending")]))),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn completed_work_runs_digest_then_validation_then_final_response_once() {
    let mut policy = FollowUpPolicy::new(true);
    let mut state = TodoSnapshot {
        todos: vec![done("ship", ConfidenceState::Plausible)],
        plan: TodoPlan {
            understands_user_intent: Some(IntentUnderstanding::Clear),
            ..Default::default()
        },
        gate_observations: vec![GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            state: Some("partial".into()),
        }],
        ..Default::default()
    };

    let digest = policy.decide(&state);
    assert_eq!(kind(&digest), Some(FollowUpKind::GateDigest));
    assert!(digest.effects.clear_gate_observations);
    assert!(is_auto_poke_message(&digest.follow_up.unwrap().message));
    state.gate_observations.clear();

    let validation = policy.decide(&state);
    assert_eq!(kind(&validation), Some(FollowUpKind::CompletionValidation));
    assert!(validation.follow_up.unwrap().message.contains("\"ship\""));

    state.todos = vec![done("ship", ConfidenceState::Verified)];
    let final_response = policy.decide(&state);
    assert_eq!(kind(&final_response), Some(FollowUpKind::FinalResponse));
    assert!(
        final_response
            .follow_up
            .unwrap()
            .notice
            .contains("verified")
    );
    // The final-response turn itself must not trigger another cycle.
    assert_eq!(policy.decide(&state), Decision::default());
    // New work starts a fresh cycle.
    state.todos.push(todo("follow-up", "pending"));
    assert_eq!(
        kind(&policy.decide(&state)),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn empty_digest_still_consumes_the_log() {
    let mut policy = FollowUpPolicy::new(true);
    let state = TodoSnapshot {
        todos: vec![done("ship", ConfidenceState::Verified)],
        gate_observations: Vec::new(),
        ..Default::default()
    };
    // No observations: nothing to clear.
    assert!(!policy.decide(&state).effects.clear_gate_observations);
}

#[test]
fn ownership_gate_asks_once_per_distinct_gap() {
    let mut policy = FollowUpPolicy::new(true);
    let state = TodoSnapshot {
        todos: vec![done("ship", ConfidenceState::Verified)],
        goals: vec![TodoGoal {
            delivery_state: Some(DeliveryState::ChangeMade),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(kind(&policy.decide(&state)), Some(FollowUpKind::Ownership));
    // Same assessment: stop instead of looping or claiming success.
    assert_eq!(policy.decide(&state), Decision::default());
}

#[test]
fn confidence_spike_is_challenged_once() {
    let mut policy = FollowUpPolicy::new(true);
    let mut spiked = done("ship", ConfidenceState::Verified);
    spiked.confidence_history = vec![ConfidenceState::Speculative, ConfidenceState::Verified];
    let state = snapshot(vec![spiked]);
    assert_eq!(
        kind(&policy.decide(&state)),
        Some(FollowUpKind::ConfidenceSpike)
    );
    assert_eq!(
        kind(&policy.decide(&state)),
        Some(FollowUpKind::FinalResponse)
    );
}

#[test]
fn long_session_review_precedes_poke_and_is_marked() {
    let mut policy = FollowUpPolicy::new(true);
    let state = TodoSnapshot {
        todos: vec![todo("a", "in_progress")],
        long_session_review_due: true,
        ..Default::default()
    };
    let decision = policy.decide(&state);
    assert_eq!(kind(&decision), Some(FollowUpKind::LongSessionReview));
    assert!(decision.effects.mark_long_session_review_delivered);
}

#[test]
fn exhausted_gate_budget_disarms_with_notice_and_rearms_for_new_work() {
    let mut policy = FollowUpPolicy::new(true);
    let mut sent = 0;
    let mut stopped = false;
    for round in 0..(MAX_GATE_ATTEMPTS as usize + 3) {
        // Keep changing wording so ownership de-dup does not stop it first,
        // while validation never passes.
        let state = snapshot(vec![done(
            &format!("ship {round}"),
            ConfidenceState::Speculative,
        )]);
        let decision = policy.decide(&state);
        if decision.follow_up.is_some() {
            sent += 1;
        }
        if decision.notice.is_some() {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "breaker must trip");
    assert_eq!(sent, MAX_GATE_ATTEMPTS as usize);
    assert_eq!(
        policy.decide(&snapshot(vec![done(
            "ship x",
            ConfidenceState::Speculative
        )])),
        Decision::default()
    );
    assert_eq!(
        kind(&policy.decide(&snapshot(vec![todo("new", "pending")]))),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn disable_sticks_until_enabled() {
    let mut policy = FollowUpPolicy::new(true);
    policy.disable();
    assert!(!policy.is_enabled());
    assert_eq!(
        policy.decide(&snapshot(vec![todo("a", "pending")])),
        Decision::default()
    );
}

#[test]
fn snapshot_round_trips_through_json() {
    let state = TodoSnapshot {
        todos: vec![todo("a", "pending")],
        long_session_review_due: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&state).unwrap();
    assert_eq!(serde_json::from_str::<TodoSnapshot>(&json).unwrap(), state);
    assert_eq!(
        serde_json::from_str::<TodoSnapshot>("{}").unwrap(),
        TodoSnapshot::default()
    );
}
