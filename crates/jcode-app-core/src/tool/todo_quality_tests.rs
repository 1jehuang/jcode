use super::*;

#[test]
fn goal_changes_include_only_updated_quality_fields() {
    let before = TodoGoal {
        group: Some("search".to_string()),
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Strong),
        feedback_loop: Some("Run one benchmark".to_string()),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Indirect),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::Narrow),
        delivery_state: None,
        ..Default::default()
    };
    let after = TodoGoal {
        closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Closed),
        feedback_loop: Some("Run five benchmarks and compare p50".to_string()),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
        ..before.clone()
    };

    let changes = goal_changes(std::slice::from_ref(&before), std::slice::from_ref(&after));

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].before.as_ref(), Some(&before));
    assert_eq!(changes[0].after.as_ref(), Some(&after));
    assert_eq!(
        changes[0].fields,
        vec![
            TodoGoalField::ClosedFeedbackLoop,
            TodoGoalField::FeedbackLoop,
            TodoGoalField::FeedbackLoopRelevance,
            TodoGoalField::FeedbackLoopCoverage,
        ]
    );
}

/// The core behavior change: a low score records an observation for the
/// turn-end digest instead of interrupting the write, and repeated writes
/// do not re-interrupt.
#[test]
fn low_open_goal_records_an_observation_without_interrupting() {
    let todos = vec![open_todo(Some("design"))];
    let plan = aligned_plan();
    let goals = vec![
        goal(Some("design"), crate::todo::FeedbackLoopState::Strong),
        goal(Some("perf"), crate::todo::FeedbackLoopState::Closed),
    ];
    let (observations, nudges) = record_reframe_observations(&plan, &goals, &todos, &[]);

    assert!(
        nudges.is_empty(),
        "a low closed feedback loop score must not interrupt the write"
    );
    assert_eq!(
        observations,
        vec![GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: Some("design".to_string()),
            state: Some("strong".to_string()),
        }]
    );
    // A subsequent write still records, still does not interrupt.
    let (again, nudges) = record_reframe_observations(&plan, &goals, &todos, &[]);
    assert_eq!(again, observations);
    assert!(nudges.is_empty());
}

#[test]
fn low_intent_is_plan_level_and_independent_of_goals() {
    let todos = vec![open_todo(Some("coverage"))];
    let plan = TodoPlan {
        user_intention: Some("partially understood".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
        understands_user_intent_history: vec![crate::todo::IntentUnderstanding::Partial],
    };
    let (observations, nudges) = record_reframe_observations(
        &plan,
        &[goal(
            Some("coverage"),
            crate::todo::FeedbackLoopState::Closed,
        )],
        &todos,
        &[],
    );

    assert_eq!(
        observations,
        vec![GateObservation {
            kind: GateObservationKind::IntentUnderstanding,
            group: None,
            state: Some("partial".to_string()),
        }]
    );
    // 95 is below threshold but nowhere near severe, so exploration is
    // given the chance to resolve it rather than being interrupted.
    assert!(nudges.is_empty());
}

/// The single retained immediate nudge: the agent's first plan write says it
/// does not understand the task at all, and a whole turn of wrong work
/// cannot be undone at turn end.
#[test]
fn severely_low_first_intent_still_nudges_immediately() {
    let todos = vec![open_todo(None)];
    let plan = TodoPlan {
        user_intention: Some("guessing".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Uncertain),
        understands_user_intent_history: vec![crate::todo::IntentUnderstanding::Uncertain],
    };
    let (_, nudges) = record_reframe_observations(&plan, &[], &todos, &[]);
    assert_eq!(nudges, vec![TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE]);
    assert!(!nudges[0].contains("40"));
    assert!(!nudges[0].to_ascii_lowercase().contains("threshold"));

    // Once the plan has a history, the same severe score is deferred to the
    // digest rather than nudged again on every write.
    let later = TodoPlan {
        understands_user_intent_history: vec![
            crate::todo::IntentUnderstanding::Uncertain,
            crate::todo::IntentUnderstanding::Uncertain,
        ],
        ..plan
    };
    let (_, nudges) = record_reframe_observations(&later, &[], &todos, &[]);
    assert!(nudges.is_empty());
}

/// Work that was already complete before this write is grandfathered: the
/// turn cannot go back and improve a loop over work it did not do.
#[test]
fn work_already_closed_before_this_write_records_nothing() {
    let mut done = open_todo(None);
    done.status = "completed".to_string();
    let already = vec![done.clone()];
    let (observations, nudges) = record_reframe_observations(
        &TodoPlan::default(),
        &[goal(None, crate::todo::FeedbackLoopState::Absent)],
        &already,
        &already,
    );
    assert!(observations.is_empty());
    assert!(nudges.is_empty());
}

/// A group created and finished in one write must still be observed. This is
/// where a weak feedback loop hides best: declare it done in one step and no
/// "still open" check ever sees it.
#[test]
fn a_group_closed_by_this_write_is_still_observed() {
    let mut done = open_todo(Some("one shot"));
    done.status = "completed".to_string();
    let (observations, nudges) = record_reframe_observations(
        &aligned_plan(),
        &[goal(Some("one shot"), crate::todo::FeedbackLoopState::Weak)],
        &[done],
        &[],
    );
    assert!(nudges.is_empty());
    assert_eq!(
        observations,
        vec![GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: Some("one shot".to_string()),
            state: Some("weak".to_string()),
        }]
    );
}

#[test]
fn both_weak_links_are_recorded_independently() {
    let todos = vec![open_todo(Some("coverage"))];
    let plan = TodoPlan {
        user_intention: Some("partially understood".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
        understands_user_intent_history: vec![crate::todo::IntentUnderstanding::Partial],
    };
    let (observations, _) = record_reframe_observations(
        &plan,
        &[goal(
            Some("coverage"),
            crate::todo::FeedbackLoopState::Strong,
        )],
        &todos,
        &[],
    );
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.kind)
            .collect::<Vec<_>>(),
        vec![
            GateObservationKind::IntentUnderstanding,
            GateObservationKind::ClosedFeedbackLoop,
        ]
    );
}

#[test]
fn missing_quality_scores_still_record_observations() {
    let todos = vec![open_todo(Some("coverage"))];
    let mut goal = goal(Some("coverage"), crate::todo::FeedbackLoopState::Closed);
    goal.closed_feedback_loop = None;

    let (observations, _) = record_reframe_observations(&TodoPlan::default(), &[goal], &todos, &[]);
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.kind)
            .collect::<Vec<_>>(),
        vec![
            GateObservationKind::IntentUnderstanding,
            GateObservationKind::ClosedFeedbackLoop,
        ]
    );
}

/// Groups already complete before this write are grandfathered, so a
/// long-lived session does not re-flag work from previous turns.
#[test]
fn observations_skip_goals_closed_in_an_earlier_write() {
    let mut done = open_todo(Some("legacy"));
    done.status = "completed".to_string();
    let already = vec![done];
    let goals = vec![goal(Some("legacy"), crate::todo::FeedbackLoopState::Absent)];
    let (observations, _) =
        record_reframe_observations(&aligned_plan(), &goals, &already, &already);
    assert!(observations.is_empty());
}

#[test]
fn observations_cover_the_ungrouped_implicit_goal() {
    let todos = vec![open_todo(None)];
    let goals = vec![goal(None, crate::todo::FeedbackLoopState::Absent)];
    let (observations, _) = record_reframe_observations(&aligned_plan(), &goals, &todos, &[]);
    assert_eq!(
        observations,
        vec![GateObservation {
            kind: GateObservationKind::ClosedFeedbackLoop,
            group: None,
            state: Some("absent".to_string()),
        }]
    );
}

/// Tool-owned histories are the substrate the turn-end digest reasons over,
/// so a model-supplied trail must not be able to fabricate a climb.
#[test]
fn plan_and_goal_score_histories_are_tool_maintained() {
    let stored = TodoPlan {
        user_intention: Some("ship it".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
        understands_user_intent_history: vec![crate::todo::IntentUnderstanding::Partial],
    };
    let merged = merge_plan(
        &stored,
        Some(TodoPlan {
            understands_user_intent: Some(crate::todo::IntentUnderstanding::Clear),
            // Forged trail: discarded in favor of the stored one.
            understands_user_intent_history: vec![
                crate::todo::IntentUnderstanding::Uncertain,
                crate::todo::IntentUnderstanding::Uncertain,
                crate::todo::IntentUnderstanding::Uncertain,
            ],
            ..Default::default()
        }),
    );
    assert_eq!(
        merged.understands_user_intent_history,
        vec![
            crate::todo::IntentUnderstanding::Partial,
            crate::todo::IntentUnderstanding::Clear
        ]
    );
    assert_eq!(merged.user_intention.as_deref(), Some("ship it"));

    // Re-sending the same state does not manufacture an extra step.
    let merged = merge_plan(
        &merged,
        Some(TodoPlan {
            understands_user_intent: Some(crate::todo::IntentUnderstanding::Clear),
            ..Default::default()
        }),
    );
    assert_eq!(
        merged.understands_user_intent_history,
        vec![
            crate::todo::IntentUnderstanding::Partial,
            crate::todo::IntentUnderstanding::Clear
        ]
    );

    let stored_goals = merge_goals(
        &[],
        Some(vec![TodoGoal {
            group: Some("perf".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Usable),
            feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Indirect),
            feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::Narrow),
            ..Default::default()
        }]),
    );
    assert_eq!(
        stored_goals[0].closed_feedback_loop_history,
        vec![crate::todo::FeedbackLoopState::Usable]
    );
    let merged_goals = merge_goals(
        &stored_goals,
        Some(vec![TodoGoal {
            group: Some("perf".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Strong),
            feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::AcceptanceAligned),
            feedback_loop_relevance_history: vec![
                crate::todo::FeedbackLoopRelevance::AcceptanceAligned,
            ],
            feedback_loop_coverage: Some(
                crate::todo::FeedbackLoopCoverage::EdgeAndIntegrationPaths,
            ),
            feedback_loop_coverage_history: vec![
                crate::todo::FeedbackLoopCoverage::EdgeAndIntegrationPaths,
            ],
            ..Default::default()
        }]),
    );
    assert_eq!(
        merged_goals[0].closed_feedback_loop_history,
        vec![
            crate::todo::FeedbackLoopState::Usable,
            crate::todo::FeedbackLoopState::Strong
        ]
    );
    assert_eq!(
        merged_goals[0].feedback_loop_relevance_history,
        vec![
            crate::todo::FeedbackLoopRelevance::Indirect,
            crate::todo::FeedbackLoopRelevance::AcceptanceAligned,
        ]
    );
    assert_eq!(
        merged_goals[0].feedback_loop_coverage_history,
        vec![
            crate::todo::FeedbackLoopCoverage::Narrow,
            crate::todo::FeedbackLoopCoverage::EdgeAndIntegrationPaths,
        ]
    );
}

/// A write that revises one assessment must not erase the others, or the
/// digest would read a stale `None` and re-raise a resolved point.
#[test]
fn omitted_goal_fields_inherit_the_stored_assessment() {
    let stored = merge_goals(
        &[],
        Some(vec![TodoGoal {
            group: Some("perf".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Closed),
            feedback_loop: Some("cargo bench".to_string()),
            feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
            feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
            delivery_state: Some(crate::todo::DeliveryState::OutcomeDelivered),
            ..Default::default()
        }]),
    );
    let merged = merge_goals(
        &stored,
        Some(vec![TodoGoal {
            group: Some("perf".to_string()),
            ..Default::default()
        }]),
    );
    assert_eq!(
        merged[0].closed_feedback_loop,
        Some(crate::todo::FeedbackLoopState::Closed)
    );
    assert_eq!(merged[0].feedback_loop.as_deref(), Some("cargo bench"));
    assert_eq!(
        merged[0].feedback_loop_relevance,
        Some(crate::todo::FeedbackLoopRelevance::Representative)
    );
    assert_eq!(
        merged[0].feedback_loop_coverage,
        Some(crate::todo::FeedbackLoopCoverage::MainPaths)
    );
    assert_eq!(
        merged[0].delivery_state,
        Some(crate::todo::DeliveryState::OutcomeDelivered)
    );
}

#[test]
fn garbage_string_still_errors() {
    assert!(parse(json!({"todos": "not json at all"})).is_err());
}

/// Sessions and model calls written before the rename carry
/// `hill_climbability`. Those must keep loading, or resuming an old session
/// silently drops its goal assessments and re-raises resolved gate points.
#[test]
fn pre_rename_hill_climbability_keys_still_load() {
    let goal: crate::todo::TodoGoal = serde_json::from_value(json!({
        "group": "optimize grep",
        "hill_climbability": 91,
        "hill_climbability_history": [70, 91],
        "feedback_loop": "cargo bench grep"
    }))
    .expect("the pre-rename key must still deserialize");
    assert_eq!(
        goal.closed_feedback_loop,
        Some(crate::todo::FeedbackLoopState::Strong)
    );
    assert_eq!(
        goal.closed_feedback_loop_history,
        vec![
            crate::todo::FeedbackLoopState::Usable,
            crate::todo::FeedbackLoopState::Strong
        ]
    );

    let goals = parse(json!({
        "goals": [{"group": "optimize grep", "hill_climbability": "88", "feedback_loop": "bench"}]
    }))
    .expect("a pre-rename tool call must still parse")
    .goals
    .expect("goals should be present");
    assert_eq!(
        goals[0].closed_feedback_loop,
        Some(crate::todo::FeedbackLoopState::Strong)
    );
}

use crate::todo::ConfidenceState as CS;

fn history_todo(id: &str, confidence: Option<CS>, history: Vec<CS>) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: format!("todo {id}"),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        confidence,
        confidence_history: history,
        ..Default::default()
    }
}

#[test]
fn confidence_history_appends_changes_and_skips_repeats() {
    let previous = vec![history_todo("1", Some(CS::Plausible), vec![CS::Plausible])];
    // Same confidence again: no new entry.
    let mut incoming = vec![history_todo("1", Some(CS::Plausible), Vec::new())];
    merge_confidence_history(&previous, &mut incoming);
    assert_eq!(incoming[0].confidence_history, vec![CS::Plausible]);
    // Raised confidence: appended.
    let mut incoming = vec![history_todo("1", Some(CS::Validated), Vec::new())];
    merge_confidence_history(&previous, &mut incoming);
    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Validated]
    );
}

#[test]
fn confidence_history_records_completion_confidence() {
    let previous = vec![history_todo("1", Some(CS::Plausible), vec![CS::Plausible])];
    let mut done = history_todo("1", Some(CS::Verified), Vec::new());
    done.status = "completed".to_string();
    done.completion_confidence = Some(CS::Verified);
    let mut incoming = vec![done];
    merge_confidence_history(&previous, &mut incoming);
    // 75 (planning) -> 100 (final bulk stamp): the spike stays visible.
    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Verified]
    );
}

#[test]
fn completion_write_contributes_only_one_final_confidence_observation() {
    let previous = vec![history_todo("1", Some(CS::Plausible), vec![CS::Plausible])];
    let mut done = history_todo("1", Some(CS::Plausible), Vec::new());
    done.status = "completed".to_string();
    done.completion_confidence = Some(CS::Verified);

    let mut incoming = vec![done];
    merge_confidence_history(&previous, &mut incoming);

    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Verified]
    );
}

#[test]
fn confidence_history_seeds_legacy_todos_before_completion() {
    let previous = vec![history_todo("1", Some(CS::Plausible), Vec::new())];
    let mut done = history_todo("1", Some(CS::Plausible), Vec::new());
    done.status = "completed".to_string();
    done.completion_confidence = Some(CS::Verified);

    let mut incoming = vec![done];
    merge_confidence_history(&previous, &mut incoming);

    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Verified]
    );
}

#[test]
fn confidence_history_ignores_model_supplied_history_for_new_todos() {
    let mut incoming = vec![history_todo(
        "9",
        Some(CS::Plausible),
        vec![CS::Speculative, CS::Verified],
    )];
    merge_confidence_history(&[], &mut incoming);
    assert_eq!(incoming[0].confidence_history, vec![CS::Plausible]);
}
