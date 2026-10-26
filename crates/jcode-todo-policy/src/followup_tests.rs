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
fn requires_user_action_pauses_retries_but_not_independent_work() {
    let mut blocked = todo("user-only", "pending");
    blocked.requires_user_action = Some(crate::TodoUserAction {
        action: "Complete identity verification".into(),
        reason: "Only the account owner may verify their identity".into(),
    });
    assert!(todo_requires_user_action(&blocked));
    let mut state = snapshot(vec![blocked]);
    state.long_session_review_due = true;
    let mut policy = FollowUpPolicy::new(true);
    for _ in 0..10 {
        assert_eq!(policy.decide(&state), Decision::default());
    }
    state.long_session_review_due = false;
    state.todos.push(todo("independent", "pending"));
    let followup = policy.decide(&state).follow_up.unwrap();
    assert!(followup.message.contains("independent"));
    assert!(!followup.message.contains("user-only"));
    state.todos.pop();
    state.todos[0].requires_user_action = None;
    assert_eq!(
        kind(&policy.decide(&state)),
        Some(FollowUpKind::IncompleteTodos)
    );
    for status in ["completed", "cancelled", "done"] {
        state.todos[0].status = status.into();
        state.todos[0].requires_user_action = Some(crate::TodoUserAction::default());
        assert!(!todo_requires_user_action(&state.todos[0]));
    }
}

#[test]
fn requires_user_action_serialization_rejects_missing_or_blank_fields() {
    let legacy =
        serde_json::json!({"id":"a", "content":"A", "status":"pending", "priority":"high"});
    assert!(
        serde_json::from_value::<TodoItem>(legacy.clone())
            .unwrap()
            .requires_user_action
            .is_none()
    );
    for required in [
        serde_json::json!({"action":"", "reason":"owner only"}),
        serde_json::json!({"action":"verify", "reason":" \n\t"}),
        serde_json::json!({"action":"verify"}),
        serde_json::json!({"reason":"owner only"}),
    ] {
        let mut value = legacy.clone();
        value["requires_user_action"] = required;
        assert!(serde_json::from_value::<TodoItem>(value).is_err());
    }
    let mut value = legacy;
    value["requires_user_action"] = serde_json::json!({"action":"verify", "reason":"owner only"});
    let item: TodoItem = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(item).unwrap()["requires_user_action"],
        value["requires_user_action"]
    );
}

#[test]
fn external_blocker_pauses_only_blocked_todos_without_spending_review_budget() {
    let now = chrono::Utc::now();
    let mut blocked = todo("await acceptance", "pending");
    blocked.external_blocker = Some(crate::TodoExternalBlocker {
        reason: "Coordinator must accept frozen source".into(),
        review_at: now + chrono::Duration::minutes(5),
    });
    let mut state = snapshot(vec![blocked.clone()]);
    state.long_session_review_due = true;
    let mut policy = FollowUpPolicy::new(true);
    for _ in 0..10 {
        assert_eq!(policy.decide(&state), Decision::default());
    }
    assert!(is_incomplete_todo(&blocked));
    assert!(todo_has_external_blocker_at(&blocked, now));
    assert!(!todo_has_external_blocker_at(
        &blocked,
        now + chrono::Duration::minutes(5)
    ));
    state.long_session_review_due = false;
    state.todos.push(todo("independent", "pending"));
    let followup = policy.decide(&state).follow_up.unwrap();
    assert!(followup.message.contains("independent"));
    assert!(!followup.message.contains("await acceptance"));
    state.todos.pop();
    state.todos[0].external_blocker.as_mut().unwrap().review_at =
        now - chrono::Duration::seconds(1);
    let followup = policy.decide(&state).follow_up.unwrap();
    assert_eq!(followup.kind, FollowUpKind::IncompleteTodos);
    assert!(followup.message.contains("await acceptance"));
    assert_eq!(
        state.todos[0].status, "pending",
        "expiry reassesses, never completes"
    );
    state.todos[0].external_blocker = None;
    assert!(!todo_has_external_blocker(&state.todos[0]));
}

#[test]
fn external_blocker_requires_typed_finite_deadline_and_preserves_legacy_todos() {
    let mut value =
        serde_json::json!({"id":"a", "content":"A", "status":"pending", "priority":"high"});
    let legacy: TodoItem = serde_json::from_value(value.clone()).unwrap();
    assert!(legacy.external_blocker.is_none());
    for blocker in [
        serde_json::json!({"reason":"wait"}),
        serde_json::json!({"review_at":"soon", "reason":"wait"}),
        serde_json::json!({"review_at":"2026-10-26T00:00:00Z"}),
    ] {
        value["external_blocker"] = blocker;
        assert!(serde_json::from_value::<TodoItem>(value.clone()).is_err());
    }
    value["external_blocker"] =
        serde_json::json!({"reason":"wait", "review_at":"2026-10-26T00:00:00Z"});
    let loaded: TodoItem = serde_json::from_value(value.clone()).unwrap();
    assert!(loaded.external_blocker.is_some());
    assert_eq!(
        serde_json::to_value(loaded).unwrap()["external_blocker"],
        value["external_blocker"]
    );
}

#[test]
fn running_dependencies_pause_without_consuming_budget_or_reviews() {
    let mut policy = FollowUpPolicy::new(true);
    let mut state = snapshot(vec![TodoItem {
        requires_user_action: None,
        external_blocker: None,
        waiting_on: vec!["build".into()],
        ..todo("validate", "in_progress")
    }]);
    // First exhaust the actionable budget, then explicitly wait. Resolution
    // must get a fresh budget even though the todo payload did not change.
    for _ in 0..INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert_eq!(
            kind(&policy.decide(&state)),
            Some(FollowUpKind::IncompleteTodos)
        );
    }
    assert_eq!(policy.decide(&state), Decision::default());
    state.running_background_tasks = vec!["build".into()];
    state.long_session_review_due = true;
    for _ in 0..10 {
        assert_eq!(policy.decide(&state), Decision::default());
    }
    state.running_background_tasks.clear();
    state.long_session_review_due = false;
    for _ in 0..INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert_eq!(
            kind(&policy.decide(&state)),
            Some(FollowUpKind::IncompleteTodos)
        );
    }
}

#[test]
fn mixed_dependencies_poke_only_actionable_todos() {
    let mut policy = FollowUpPolicy::new(true);
    let mut state = snapshot(vec![
        TodoItem {
            requires_user_action: None,
            external_blocker: None,
            waiting_on: vec!["build".into()],
            ..todo("blocked-validation", "pending")
        },
        todo("write-tests", "in_progress"),
    ]);
    state.running_background_tasks = vec!["build".into()];
    let decision = policy.decide(&state);
    let follow_up = decision
        .follow_up
        .expect("independent work should continue");
    assert_eq!(follow_up.kind, FollowUpKind::IncompleteTodos);
    assert!(follow_up.message.starts_with("You have 1 incomplete todo."));
    assert!(follow_up.message.contains("write-tests"));
    assert!(!follow_up.message.contains("blocked-validation"));
    assert!(is_auto_poke_message(&follow_up.message));
}

#[test]
fn dependencies_only_block_when_runtime_confirms_running() {
    let todo = TodoItem {
        requires_user_action: None,
        external_blocker: None,
        waiting_on: vec!["first".into(), "second".into()],
        ..todo("work", "pending")
    };
    assert!(!todo_waits_on_running_task(&todo, |_| false));
    assert!(todo_waits_on_running_task(&todo, |id| id == "second"));
    assert!(!todo_waits_on_running_task(&todo, |id| id == "unrelated"));
    let mut policy = FollowUpPolicy::new(true);
    assert_eq!(
        kind(&policy.decide(&snapshot(vec![todo]))),
        Some(FollowUpKind::IncompleteTodos)
    );
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
fn pokes_open_work_three_times_per_distinct_state() {
    let mut policy = FollowUpPolicy::new(true);
    let open = snapshot(vec![todo("a", "pending"), todo("b", "in_progress")]);
    let first = policy.decide(&open);
    let follow_up = first.follow_up.expect("poke");
    assert_eq!(follow_up.kind, FollowUpKind::IncompleteTodos);
    assert!(is_auto_poke_message(&follow_up.message));
    assert!(follow_up.notice.contains("2 incomplete todos"));
    for _ in 1..INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert_eq!(
            kind(&policy.decide(&open)),
            Some(FollowUpKind::IncompleteTodos)
        );
    }
    // Three attempts without progress exhaust the budget, without disabling it.
    assert_eq!(policy.decide(&open), Decision::default());
    assert_eq!(policy.decide(&open), Decision::default());
    assert!(policy.is_enabled());
    // Progress re-arms the poke.
    let progressed = snapshot(vec![todo("a", "completed"), todo("b", "in_progress")]);
    assert_eq!(
        kind(&policy.decide(&progressed)),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn incomplete_budget_resets_after_clear_and_explicit_cycle_reset() {
    let open = snapshot(vec![todo("a", "pending")]);
    let mut policy = FollowUpPolicy::new(true);
    for clear in [
        TodoSnapshot::default(),
        snapshot(vec![done("a", ConfidenceState::Verified)]),
    ] {
        for _ in 0..INCOMPLETE_TODO_MAX_ATTEMPTS {
            assert_eq!(
                kind(&policy.decide(&open)),
                Some(FollowUpKind::IncompleteTodos)
            );
        }
        assert_eq!(policy.decide(&open), Decision::default());
        policy.decide(&clear);
    }
    for _ in 0..INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert_eq!(
            kind(&policy.decide(&open)),
            Some(FollowUpKind::IncompleteTodos)
        );
    }
    assert_eq!(policy.decide(&open), Decision::default());
    policy.reset_cycle();
    assert_eq!(
        kind(&policy.decide(&open)),
        Some(FollowUpKind::IncompleteTodos)
    );
    policy.disable();
    policy.reset_cycle();
    assert_eq!(policy.decide(&open), Decision::default());
    let changed = snapshot(vec![todo("changed", "pending")]);
    assert_eq!(policy.decide(&changed), Decision::default());
}

#[test]
fn incomplete_budget_tracks_open_item_state_not_just_count() {
    let mut budget = IncompleteTodoBudget::default();
    let mut todos = vec![todo("a", "pending"), done("b", ConfidenceState::Verified)];
    for _ in 0..INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert!(budget.allow_follow_up(&todos));
    }
    assert!(!budget.allow_follow_up(&todos));
    // Unrelated completed-item wording does not buy more retries.
    todos[1].content = "new completed wording".into();
    assert!(!budget.allow_follow_up(&todos));
    todos[0].status = "in_progress".into();
    for _ in 0..INCOMPLETE_TODO_MAX_ATTEMPTS {
        assert!(budget.allow_follow_up(&todos));
    }
    assert!(!budget.allow_follow_up(&todos));
    todos[0].status = "cancelled".into();
    assert!(budget.allow_follow_up(&todos));
    todos[0].status = "in_progress".into();
    assert!(budget.allow_follow_up(&todos));
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
fn completed_work_skips_digest_and_runs_validation_then_final_response_once() {
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

    let validation = policy.decide(&state);
    assert!(validation.effects.clear_gate_observations);
    state.gate_observations.clear();
    assert_eq!(kind(&validation), Some(FollowUpKind::CompletionValidation));
    assert!(validation.follow_up.unwrap().message.contains("\"ship\""));

    state.todos = vec![done("ship", ConfidenceState::Verified)];
    let final_response = policy.decide(&state);
    assert_eq!(kind(&final_response), Some(FollowUpKind::FinalResponse));
    assert!(final_response.follow_up.unwrap().notice.is_empty());
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
fn weak_points_do_not_delay_completed_work() {
    let mut policy = FollowUpPolicy::new(true);
    let state = TodoSnapshot {
        todos: vec![done("ship", ConfidenceState::Verified)],
        gate_observations: [
            GateObservationKind::IntentUnderstanding,
            GateObservationKind::ClosedFeedbackLoop,
            GateObservationKind::FeedbackLoopRelevance,
            GateObservationKind::FeedbackLoopCoverage,
            GateObservationKind::FeedbackLoopTraceability,
        ]
        .into_iter()
        .map(|kind| GateObservation {
            kind,
            group: None,
            state: None,
        })
        .collect(),
        ..Default::default()
    };
    let decision = policy.decide(&state);
    assert_eq!(kind(&decision), Some(FollowUpKind::FinalResponse));
    assert!(decision.effects.clear_gate_observations);
    // Even an unacknowledged observation log cannot restart the cycle.
    assert_eq!(policy.decide(&state), Decision::default());
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
fn opted_in_digest_runs_once_and_preserves_validation() {
    let mut policy = FollowUpPolicy::new(true);
    policy.set_gate_digest_enabled(true);
    let state = TodoSnapshot {
        todos: vec![done("ship", ConfidenceState::Plausible)],
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
    assert!(
        digest
            .follow_up
            .unwrap()
            .message
            .starts_with(crate::TODO_GATE_DIGEST_PREFIX)
    );
    // Even an unacknowledged/repeated log must not cause another digest.
    assert_eq!(
        kind(&policy.decide(&state)),
        Some(FollowUpKind::CompletionValidation)
    );
    assert_eq!(
        kind(&policy.decide(&snapshot(vec![todo("new", "pending")]))),
        Some(FollowUpKind::IncompleteTodos)
    );
    assert_eq!(kind(&policy.decide(&state)), Some(FollowUpKind::GateDigest));
    assert_eq!(policy.decide(&TodoSnapshot::default()), Decision::default());
    assert_eq!(kind(&policy.decide(&state)), Some(FollowUpKind::GateDigest));
    policy.reset_cycle();
    assert_eq!(kind(&policy.decide(&state)), Some(FollowUpKind::GateDigest));
    policy.disable();
    assert_eq!(policy.decide(&state), Decision::default());
    policy.enable();
    assert_eq!(kind(&policy.decide(&state)), Some(FollowUpKind::GateDigest));
    policy.set_gate_digest_enabled(false);
    policy.reset_cycle();
    assert_eq!(
        kind(&policy.decide(&state)),
        Some(FollowUpKind::CompletionValidation)
    );
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

#[test]
fn delivered_final_latches_completion_without_requesting_another_response() {
    let mut policy = FollowUpPolicy::new(true);
    let mut state = snapshot(vec![done("ship", ConfidenceState::Verified)]);
    state.gate_observations.push(GateObservation {
        kind: GateObservationKind::IntentUnderstanding,
        group: None,
        state: Some("partial".into()),
    });
    let decision = policy.decide_after_turn(&state, true);
    assert_eq!(decision.follow_up, None);
    assert!(decision.notice.is_none());
    assert!(decision.effects.clear_gate_observations);
    assert!(policy.final_response_requested);
    assert_eq!(
        policy.final_response_fingerprint,
        serde_json::to_string(&(&state.todos, &state.plan, &state.goals)).ok()
    );
    // A later turn without a response or with stale observations stays quiet.
    assert_eq!(policy.decide_after_turn(&state, false), Decision::default());
    assert_eq!(policy.decide_after_turn(&state, true), Decision::default());
    assert_eq!(policy.decide(&state), Decision::default());

    // Changed completed todos start a fresh cycle and can request a response.
    state
        .todos
        .push(done("follow-up", ConfidenceState::Verified));
    assert_eq!(
        kind(&policy.decide_after_turn(&state, false)),
        Some(FollowUpKind::FinalResponse)
    );
    state.todos.push(todo("new work", "pending"));
    assert_eq!(
        kind(&policy.decide_after_turn(&state, true)),
        Some(FollowUpKind::IncompleteTodos)
    );
}

#[test]
fn absent_final_response_preserves_legacy_decision() {
    let state = snapshot(vec![done("ship", ConfidenceState::Verified)]);
    let mut legacy = FollowUpPolicy::new(true);
    let mut explicit = FollowUpPolicy::new(true);
    let decision = explicit.decide_after_turn(&state, false);
    assert_eq!(kind(&decision), Some(FollowUpKind::FinalResponse));
    assert_eq!(decision, legacy.decide(&state));
    assert_eq!(explicit, legacy);
}

#[test]
fn delivered_final_does_not_bypass_work_or_quality_gates() {
    let mut review = snapshot(vec![todo("work", "pending")]);
    review.long_session_review_due = true;
    let mut ownership = snapshot(vec![done("ship", ConfidenceState::Verified)]);
    ownership.goals.push(TodoGoal {
        delivery_state: Some(DeliveryState::ChangeMade),
        ..Default::default()
    });
    let mut spike = done("ship", ConfidenceState::Verified);
    spike.confidence_history = vec![ConfidenceState::Speculative, ConfidenceState::Verified];
    let mut digest = snapshot(vec![done("ship", ConfidenceState::Verified)]);
    digest.gate_observations.push(GateObservation {
        kind: GateObservationKind::IntentUnderstanding,
        group: None,
        state: Some("partial".into()),
    });
    for (state, expected) in [
        (review, FollowUpKind::LongSessionReview),
        (
            snapshot(vec![todo("work", "pending")]),
            FollowUpKind::IncompleteTodos,
        ),
        (ownership, FollowUpKind::Ownership),
        (
            snapshot(vec![done("ship", ConfidenceState::Plausible)]),
            FollowUpKind::CompletionValidation,
        ),
        (snapshot(vec![spike]), FollowUpKind::ConfidenceSpike),
        (digest, FollowUpKind::GateDigest),
    ] {
        let mut legacy = FollowUpPolicy::new(true);
        legacy.set_gate_digest_enabled(true);
        let mut delivered = legacy.clone();
        let decision = delivered.decide_after_turn(&state, true);
        assert_eq!(kind(&decision), Some(expected));
        assert_eq!(decision, legacy.decide(&state));
        assert_eq!(delivered, legacy);
        assert!(!delivered.final_response_requested);
        assert!(delivered.final_response_fingerprint.is_none());
    }
}

#[test]
fn delivered_final_preserves_gate_breaker_and_disabled_policy() {
    let state = snapshot(vec![done("ship", ConfidenceState::Speculative)]);
    let mut policy = FollowUpPolicy::new(true);
    for _ in 0..MAX_GATE_ATTEMPTS {
        assert_eq!(
            kind(&policy.decide_after_turn(&state, true)),
            Some(FollowUpKind::CompletionValidation)
        );
    }
    let stopped = policy.decide_after_turn(&state, true);
    assert!(stopped.notice.is_some());
    assert!(stopped.follow_up.is_none());
    assert!(!policy.final_response_requested);
    assert!(policy.final_response_fingerprint.is_none());
    policy.disable();
    assert_eq!(policy.decide_after_turn(&state, true), Decision::default());
}
